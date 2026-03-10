use alloy::providers::{Provider, ProviderBuilder};
use alloy_primitives::{Address, U256};
use alloy_sol_types::SolValue;
use anyhow::{Context, Result};
use boundless_market::deployments::Deployment;
use clap::Parser;
use market_contribution_calculator::{
    op_chain_spec_from_id, IBoundlessMarket, MarketContributionJournal,
    MARKET_CONTRIBUTION_CALCULATOR_GUEST_ELF,
};
use op_alloy_network::Optimism;
use risc0_op_steel::optimism::OpEvmFactory;
use risc0_steel::{host::HostMultiblockEvmEnv, Contract, EvmEnv, MultiblockEvmInput};
use risc0_zkvm::{default_executor, ExecutorEnv};
use tracing_subscriber::EnvFilter;
use url::Url;

#[derive(Parser)]
#[command(name = "market-contribution-calculator")]
#[command(about = "Calculate market contributions for all provers across an epoch range")]
struct Args {
    /// Ethereum RPC URL
    #[arg(long, env = "RPC_URL")]
    rpc_url: Url,

    /// Start block for the epoch range (storage snapshot before the range)
    #[arg(long)]
    start_block: u64,

    /// End block for the epoch range (storage snapshot at end of range)
    #[arg(long)]
    end_block: u64,

    /// Epoch start timestamp to include in the journal
    #[arg(long)]
    epoch_start: u64,

    /// Epoch end timestamp to include in the journal
    #[arg(long)]
    epoch_end: u64,

    /// Override the BoundlessMarket contract address (auto-detected from chain ID by default)
    #[arg(long, env = "MARKET_ADDRESS")]
    market_address: Option<Address>,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt().with_env_filter(EnvFilter::from_default_env()).init();

    let args = Args::parse();

    let provider = ProviderBuilder::new()
        .disable_recommended_fillers()
        .network::<Optimism>()
        .connect_http(args.rpc_url);

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

    println!(
        "Chain ID: {chain_id}, Market: {market_address}, Blocks: {}..={}",
        args.start_block, args.end_block
    );

    // Build the multiblock EVM environment with start and end blocks.
    let builder =
        EvmEnv::<(), OpEvmFactory, ()>::builder().provider(provider.clone()).chain_spec(chain_spec);
    let mut multi_env: HostMultiblockEvmEnv<Optimism, _, OpEvmFactory, _> = builder.build_multi();

    // Read the prover count from the end block to know how many storage reads to preflight.
    let end_env = multi_env
        .get_or_build(args.end_block)
        .await
        .context("failed to build env for end block")?;

    // Preflight contributingProversLength at end block.
    let mut end_contract = Contract::preflight(market_address, end_env);
    let prover_count: U256 = end_contract
        .call_builder(&IBoundlessMarket::contributingProversLengthCall {})
        .call()
        .await
        .context("failed to preflight contributingProversLength")?;

    let count = prover_count.to::<u64>();
    println!("Contributing provers count: {count}");

    // Preflight all array reads and cumulative contribution reads at end block.
    let mut prover_addresses = Vec::with_capacity(count as usize);
    for i in 0..count {
        let index = U256::from(i);
        let prover: Address = end_contract
            .call_builder(&IBoundlessMarket::contributingProversCall { index })
            .call()
            .await
            .with_context(|| format!("failed to preflight contributingProvers({i})"))?;

        end_contract
            .call_builder(
                &IBoundlessMarket::proverCumulativeContributionCall { prover },
            )
            .call()
            .await
            .with_context(|| {
                format!("failed to preflight proverCumulativeContribution({prover}) at end block")
            })?;

        prover_addresses.push(prover);
    }

    // Preflight cumulative contribution reads at start block.
    let start_env = multi_env
        .get_or_build(args.start_block)
        .await
        .context("failed to build env for start block")?;

    let mut start_contract = Contract::preflight(market_address, start_env);
    for &prover in &prover_addresses {
        start_contract
            .call_builder(
                &IBoundlessMarket::proverCumulativeContributionCall { prover },
            )
            .call()
            .await
            .with_context(|| {
                format!("failed to preflight proverCumulativeContribution({prover}) at start block")
            })?;
    }

    // Build the multiblock input.
    println!("Building multiblock input...");
    let multi_input: MultiblockEvmInput<OpEvmFactory> =
        multi_env.into_input().await.context("failed to build multiblock input")?;

    // Build the executor environment and run the guest.
    let epoch_start = U256::from(args.epoch_start);
    let epoch_end = U256::from(args.epoch_end);
    let exec_env = ExecutorEnv::builder()
        .write(&market_address)?
        .write(&epoch_start)?
        .write(&epoch_end)?
        .write(&args.start_block)?
        .write(&args.end_block)?
        .write(&chain_id)?
        .write(&multi_input)?
        .build()
        .context("failed to build ExecutorEnv")?;

    println!("Executing guest...");
    let session_info = default_executor()
        .execute(exec_env, MARKET_CONTRIBUTION_CALCULATOR_GUEST_ELF)
        .context("guest execution failed")?;

    println!("Cycles: {:?}", session_info.cycles());

    // Decode and print the journal.
    let journal = MarketContributionJournal::abi_decode(&session_info.journal.bytes)
        .context("failed to decode journal")?;

    println!("=== Market Contribution Result ===");
    println!("Market:      {}", journal.marketAddress);
    println!("Epoch range: {} - {}", journal.epochStart, journal.epochEnd);
    println!("Contributions ({}):", journal.contributions.len());
    for c in &journal.contributions {
        println!("  {}: {}", c.prover, c.contribution);
    }

    Ok(())
}
