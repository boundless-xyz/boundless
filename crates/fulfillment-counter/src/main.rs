use std::collections::BTreeSet;

use alloy::providers::{Provider, ProviderBuilder};
use alloy_primitives::Address;
use alloy_sol_types::{SolEvent, SolValue};
use anyhow::{bail, Context, Result};
use boundless_market::deployments::Deployment;
use clap::Parser;
use fulfillment_counter::{
    op_chain_spec_from_id, FulfillmentCounterJournal, IBoundlessMarket,
    FULFILLMENT_COUNTER_GUEST_ELF,
};
use op_alloy_network::Optimism;
use risc0_op_steel::optimism::OpEvmFactory;
use risc0_steel::{host::HostMultiblockEvmEnv, Event, EvmEnv, MultiblockEvmInput};
use risc0_zkvm::{default_executor, ExecutorEnv};
use tracing_subscriber::EnvFilter;
use url::Url;

#[derive(Parser)]
#[command(name = "fulfillment-counter")]
#[command(about = "Count RequestFulfilled events for a prover across a block range")]
struct Args {
    /// Ethereum RPC URL
    #[arg(long, env = "RPC_URL")]
    rpc_url: Url,

    /// Prover address to count fulfillments for
    #[arg(long, env = "PROVER_ADDRESS")]
    prover_address: Address,

    /// How far back to look, e.g. "1h", "6h", "1d", "7d".
    /// Converted to a block range ending at the latest block.
    #[arg(long, conflicts_with_all = ["from_block", "to_block"])]
    duration: Option<String>,

    /// Start of block range (inclusive). Use with --to-block as an alternative to --duration.
    #[arg(long, requires = "to_block")]
    from_block: Option<u64>,

    /// End of block range (inclusive). Use with --from-block as an alternative to --duration.
    #[arg(long, requires = "from_block")]
    to_block: Option<u64>,

    /// Override the BoundlessMarket contract address (auto-detected from chain ID by default)
    #[arg(long, env = "MARKET_ADDRESS")]
    market_address: Option<Address>,
}

/// Parse a human-friendly duration string into seconds.
/// Supports: "30m", "1h", "6h", "1d", "7d", "2w" (minutes, hours, days, weeks).
fn parse_duration_secs(s: &str) -> Result<u64> {
    let s = s.trim();
    let (num, suffix) = s.split_at(
        s.find(|c: char| !c.is_ascii_digit())
            .context("duration must have a unit suffix (m, h, d, w)")?,
    );
    let n: u64 = num.parse().context("invalid number in duration")?;
    match suffix {
        "m" => Ok(n * 60),
        "h" => Ok(n * 3600),
        "d" => Ok(n * 86400),
        "w" => Ok(n * 604800),
        _ => bail!("unknown duration unit '{suffix}'; expected m, h, d, or w"),
    }
}

/// Binary search for the block whose timestamp is closest to (but >= ) `target_timestamp`.
async fn find_block_by_timestamp(
    provider: &impl Provider<Optimism>,
    target_timestamp: u64,
    latest_block: u64,
) -> Result<u64> {
    let mut lo = 0u64;
    let mut hi = latest_block;

    while lo < hi {
        let mid = lo + (hi - lo) / 2;
        let block = provider
            .get_block_by_number(mid.into())
            .await
            .with_context(|| format!("failed to fetch block {mid}"))?
            .with_context(|| format!("block {mid} not found"))?;

        if block.header.timestamp < target_timestamp {
            lo = mid + 1;
        } else {
            hi = mid;
        }
    }
    Ok(lo)
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt().with_env_filter(EnvFilter::from_default_env()).init();

    let args = Args::parse();

    // Use an OP-network provider for Base / OP Stack chains.
    // We only read data, so disable fillers that conflict with OP transaction types.
    let provider = ProviderBuilder::new()
        .disable_recommended_fillers()
        .network::<Optimism>()
        .connect_http(args.rpc_url);

    // Query chain ID and resolve the deployment.
    let chain_id = provider.get_chain_id().await.context("failed to get chain id")?;

    let market_address = match args.market_address {
        Some(addr) => addr,
        None => {
            let deployment = Deployment::from_chain_id(chain_id).with_context(|| {
                format!(
                    "no known deployment for chain id {chain_id}; pass --market-address explicitly"
                )
            })?;
            deployment.boundless_market_address
        }
    };

    let chain_spec = op_chain_spec_from_id(chain_id)
        .with_context(|| format!("no chain spec for chain id {chain_id}; unsupported chain"))?;

    // Resolve the block range.
    let (from_block, to_block) = match (&args.duration, args.from_block, args.to_block) {
        (Some(dur), None, None) => {
            let duration_secs = parse_duration_secs(dur)?;
            let latest = provider.get_block_number().await.context("failed to get latest block")?;
            let latest_block = provider
                .get_block_by_number(latest.into())
                .await
                .context("failed to fetch latest block")?
                .context("latest block not found")?;
            let target_ts = latest_block.header.timestamp.saturating_sub(duration_secs);
            let from = find_block_by_timestamp(&provider, target_ts, latest).await?;
            (from, latest)
        }
        (None, Some(from), Some(to)) => (from, to),
        _ => bail!("specify either --duration or both --from-block and --to-block"),
    };

    println!("Chain ID: {chain_id}, Market: {market_address}, Blocks: {from_block}..={to_block}");

    // Discover which blocks contain any RequestFulfilled events (unfiltered by prover).
    // The guest needs all events per block for future hash-chain verification;
    // prover filtering is done only inside the guest.
    let filter = alloy::rpc::types::Filter::new()
        .address(market_address)
        .topic2(args.prover_address)
        .event_signature(IBoundlessMarket::RequestFulfilled::SIGNATURE_HASH)
        .from_block(from_block)
        .to_block(to_block);

    let logs = provider.get_logs(&filter).await.context("failed to fetch logs")?;

    let relevant_blocks: BTreeSet<u64> = logs.iter().filter_map(|log| log.block_number).collect();

    if relevant_blocks.is_empty() {
        println!("No RequestFulfilled events in blocks {from_block}..={to_block}");
        return Ok(());
    }

    println!("Found {} events across {} blocks", logs.len(), relevant_blocks.len());

    // Build the multiblock EVM environment using Steel's native multiblock API with OP types.
    let builder =
        EvmEnv::<(), OpEvmFactory, ()>::builder().provider(provider.clone()).chain_spec(chain_spec);
    let mut multi_env: HostMultiblockEvmEnv<Optimism, _, OpEvmFactory, _> = builder.build_multi();

    // Insert each relevant block and preflight the event queries.
    let total_blocks = relevant_blocks.len();
    for (i, &block_number) in relevant_blocks.iter().enumerate() {
        let env = multi_env
            .get_or_build(block_number)
            .await
            .with_context(|| format!("failed to build env for block {block_number}"))?;

        Event::preflight::<IBoundlessMarket::RequestFulfilled>(env)
            .address(market_address)
            .topic2(args.prover_address)
            .query()
            .await
            .with_context(|| {
                format!("failed to preflight RequestFulfilled events at block {block_number}")
            })?;

        if (i + 1) % 20 == 0 || i + 1 == total_blocks {
            println!("Preflighted {}/{total_blocks} blocks", i + 1);
        }
    }

    // Build the multiblock env (verifies continuity) and convert to input.
    println!("Building multiblock input...");
    let multi_input: MultiblockEvmInput<OpEvmFactory> =
        multi_env.into_input().await.context("failed to build multiblock input")?;

    // Build the executor environment and run the guest.
    let exec_env = ExecutorEnv::builder()
        .write(&args.prover_address)?
        .write(&market_address)?
        .write(&chain_id)?
        .write(&multi_input)?
        .build()
        .context("failed to build ExecutorEnv")?;

    println!("Executing guest...");
    let session_info = default_executor()
        .execute(exec_env, FULFILLMENT_COUNTER_GUEST_ELF)
        .context("guest execution failed")?;

    println!("Cycles: {:?}", session_info.cycles());

    // Decode and print the journal.
    let journal = FulfillmentCounterJournal::abi_decode(&session_info.journal.bytes)
        .context("failed to decode journal")?;

    println!("=== Fulfillment Counter Result ===");
    println!("Prover:      {}", journal.prover);
    println!("Market:      {}", journal.market);
    println!("Block range: {} - {}", journal.startBlock, journal.endBlock);
    println!("Count:       {}", journal.count);

    Ok(())
}
