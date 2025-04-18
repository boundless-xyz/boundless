// Copyright (c) 2025 RISC Zero, Inc.
//
// All rights reserved.

use std::cmp::min;

use alloy::{
    network::{Ethereum, EthereumWallet},
    primitives::{
        utils::{format_units, parse_ether},
        Address, U256,
    },
    providers::{Provider, ProviderBuilder},
    signers::local::PrivateKeySigner,
};
use anyhow::{anyhow, bail, Result};
use boundless_market::{
    balance_alerts_layer::BalanceAlertConfig,
    client::{Client, ClientBuilder},
    contracts::{Input, Offer, Predicate, ProofRequest, Requirements},
    storage::{StorageProvider, StorageProviderConfig},
};
use clap::Parser;
use reth_chainspec::NamedChain;
use risc0_zkvm::{default_executor, sha::Digestible};
use tokio::time::Duration;
use tracing::level_filters::LevelFilter;
use tracing_subscriber::{fmt, prelude::*, EnvFilter};
use url::Url;
use zeth::cli::BuildArgs;
use zeth_guests::{ZETH_GUESTS_RETH_ETHEREUM_ELF, ZETH_GUESTS_RETH_ETHEREUM_ID};
use zeth_preflight::BlockBuilder;
use zeth_preflight_ethereum::RethBlockBuilder;

const RETRY_DELAY_SECS: u64 = 5;

/// An estimated upper bound on the cost of locking an fulfilling a request.
/// TODO: Make this configurable.
const LOCK_FULFILL_GAS_UPPER_BOUND: u128 = 1_000_000;

/// Arguments of order-generator-zeth CLI.
#[derive(Parser, Debug)]
#[clap(author, version, about, long_about = None)]
struct Args {
    /// URL of the Ethereum RPC endpoint.
    #[clap(short, long, env)]
    rpc_url: Url,
    /// URL of the offchain order stream endpoint.
    #[clap(short, long, env)]
    order_stream_url: Option<Url>,
    /// Storage provider to use
    #[clap(flatten)]
    storage_config: Option<StorageProviderConfig>,
    /// Private key used to interact with the BoundlessMarket contract.
    #[clap(long, env)]
    private_key: PrivateKeySigner,
    /// Address of the SetVerifier contract.
    #[clap(short, long, env)]
    set_verifier_address: Address,
    /// Address of the BoundlessMarket contract.
    #[clap(short, long, env)]
    boundless_market_address: Address,
    /// URL of the Ethereum RPC endpoint for Zeth.
    #[clap(short, long, env)]
    zeth_rpc_url: Url,
    /// Block number to start from.
    ///
    /// If not provided, the current block number will be used.
    #[clap(long)]
    start_block: Option<u64>,
    /// Number of blocks to build.
    #[clap(long, default_value = "1")]
    block_count: u64,
    /// Interval in seconds between requests.
    #[clap(long, default_value = "1800")] // 30 minutes
    interval: u64,
    /// One shot request for a specific block number.
    #[clap(long)]
    one_shot: bool,
    /// Minimum price per mcycle in ether.
    #[clap(long = "min", value_parser = parse_ether, default_value = "0.00001")]
    min_price_per_mcycle: U256,
    /// Maximum price per mcycle in ether.
    #[clap(long = "max", value_parser = parse_ether, default_value = "0.000011")]
    max_price_per_mcycle: U256,
    /// Number of seconds before the request lock-in expires.
    #[clap(long, default_value = "6000")]
    lock_timeout: u32,
    /// Number of seconds, from the bidding start, before the bid expires.
    #[clap(long, default_value = "12000")]
    timeout: u32,
    /// Ramp-up period in seconds.
    ///
    /// The bid price will increase linearly from `min_price` to `max_price` over this period.
    #[clap(long, default_value = "240")] // 240s = ~20 Sepolia blocks
    ramp_up: u32,
    /// Amount of stake tokens required, in HP.
    #[clap(long, value_parser = parse_ether, default_value = "5")]
    stake: U256,
    /// Submit the request offchain.
    #[clap(long)]
    offchain: bool,
    #[clap(long, default_value = "3")]
    max_retries: u32,
    /// Balance threshold at which to log a warning.
    #[clap(long, value_parser = parse_ether, default_value = "1")]
    warn_balance_below: Option<U256>,
    /// Balance threshold at which to log an error.
    #[clap(long, value_parser = parse_ether, default_value = "0.1")]
    error_balance_below: Option<U256>,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::registry()
        .with(fmt::layer())
        .with(
            EnvFilter::builder().with_default_directive(LevelFilter::INFO.into()).from_env_lossy(),
        )
        .init();

    match dotenvy::dotenv() {
        Ok(path) => tracing::debug!("Loaded environment variables from {:?}", path),
        Err(e) if e.not_found() => tracing::debug!("No .env file found"),
        Err(e) => bail!("failed to load .env file: {}", e),
    }

    let args = Args::parse();

    let wallet = EthereumWallet::from(args.private_key.clone());

    let balance_alerts = BalanceAlertConfig {
        watch_address: wallet.default_signer().address(),
        warn_threshold: args.warn_balance_below,
        error_threshold: args.error_balance_below,
    };

    let provider = ProviderBuilder::new().wallet(wallet).on_http(args.zeth_rpc_url.clone());
    let rpc = Some(args.zeth_rpc_url.to_string());
    let chain_id = provider.get_chain_id().await?;
    let chain = Some(NamedChain::try_from(chain_id).map_err(|_| anyhow!("Unknown chain"))?);

    let boundless_client = ClientBuilder::new()
        .with_rpc_url(args.rpc_url)
        .with_boundless_market_address(args.boundless_market_address)
        .with_set_verifier_address(args.set_verifier_address)
        .with_order_stream_url(args.order_stream_url)
        .with_storage_provider_config(args.storage_config.clone())
        .await?
        .with_private_key(args.private_key)
        .with_balance_alerts(balance_alerts)
        .build()
        .await?;

    // Upload the ZETH_GUESTS_RETH_ETHEREUM ELF to the storage provider so that it can be fetched by the market.
    let image_url = boundless_client.upload_image(ZETH_GUESTS_RETH_ETHEREUM_ELF).await?;
    tracing::info!("Uploaded image to {}", image_url);

    let mut block_number = args.start_block.unwrap_or(provider.get_block_number().await?);
    let mut ticker = tokio::time::interval(Duration::from_secs(args.interval));
    let mut consecutive_failures = 0;

    loop {
        // Attempt to get the current block number.
        let current_block = match provider.get_block_number().await {
            Ok(number) => number,
            Err(err) => {
                if let Err(e) = handle_failure(
                    &mut consecutive_failures,
                    format!("Failed to get block number: {}", err),
                    args.max_retries,
                )
                .await
                {
                    break Err(e);
                }
                continue;
            }
        };

        // Ensure that the chain has advanced enough.
        if current_block < block_number {
            if let Err(e) = handle_failure(
                &mut consecutive_failures,
                "Current block is behind expected block",
                args.max_retries,
            )
            .await
            {
                break Err(e);
            }
            continue;
        }

        // Determine how many blocks to process.
        let block_count = min(current_block - block_number + 1, args.block_count);
        let build_args =
            BuildArgs { block_number, block_count, cache: None, rpc: rpc.clone(), chain };

        let params = RequestParams {
            image_url: image_url.clone(),
            min: args.min_price_per_mcycle,
            max: args.max_price_per_mcycle,
            ramp_up: args.ramp_up,
            timeout: args.timeout,
            lock_timeout: args.lock_timeout,
            stake: args.stake,
            offchain: args.offchain,
        };
        // Attempt to submit a request.
        match submit_request(build_args, chain_id, boundless_client.clone(), params).await {
            Ok(request_id) => {
                consecutive_failures = 0; // Reset on success.
                tracing::info!(
                    "Request for blocks {} - {} submitted via 0x{:x}",
                    block_number,
                    block_number + block_count - 1,
                    request_id
                );
            }
            Err(err) => {
                let err_str = err.to_string();
                // Check for unrecoverable errors.
                if err_str.contains("insufficient funds")
                    || err_str.contains("gas required exceeds allowance")
                {
                    tracing::error!("Exiting due to unrecoverable error: {}", err);
                    break Err(err);
                }
                if let Err(e) = handle_failure(
                    &mut consecutive_failures,
                    format!(
                        "Failed to submit request for blocks {} - {}: {}",
                        block_number,
                        block_number + block_count - 1,
                        err
                    ),
                    args.max_retries,
                )
                .await
                {
                    break Err(e);
                }
                continue;
            }
        }

        // Move the window forward.
        block_number += block_count;

        // In one-shot mode, exit after a successful submission.
        if args.one_shot {
            break Ok(());
        }

        ticker.tick().await;
    }
}

struct RequestParams {
    image_url: Url,
    min: U256,
    max: U256,
    ramp_up: u32,
    timeout: u32,
    lock_timeout: u32,
    stake: U256,
    offchain: bool,
}

async fn submit_request<P, S>(
    build_args: BuildArgs,
    chain_id: u64,
    boundless_client: Client<P, S>,
    params: RequestParams,
) -> Result<U256>
where
    P: Provider<Ethereum> + 'static + Clone,
    S: StorageProvider + Clone,
{
    // preflight the block building process
    tracing::info!("Building for block {} ...", build_args.block_number);
    let build_result = RethBlockBuilder::build_blocks(
        Some(chain_id),
        None,
        build_args.rpc.clone(),
        build_args.block_number,
        build_args.block_count,
    )
    .await?;

    let guest_env = Input::builder()
        .write_frame(&build_result.encoded_rkyv_input)
        .write_frame(&build_result.encoded_chain_input)
        .build_env()?;
    let input_url = boundless_client.upload_input(&guest_env.encode()?).await?;
    tracing::info!("Uploaded input to {}", input_url);

    tracing::info!("Executing for block {} ...", build_args.block_number);
    // run executor only
    let session_info =
        default_executor().execute(guest_env.try_into()?, ZETH_GUESTS_RETH_ETHEREUM_ELF)?;

    let cycles_count = session_info.segments.iter().map(|segment| 1 << segment.po2).sum::<u64>();
    let min_price =
        params.min.checked_mul(U256::from(cycles_count)).unwrap().div_ceil(U256::from(1_000_000));
    let mcycle_max_price =
        params.max.checked_mul(U256::from(cycles_count)).unwrap().div_ceil(U256::from(1_000_000));

    tracing::info!(
        "{} cycles count {} mcycles count {} min_price in ether {} mcycle_max_price in ether",
        cycles_count,
        cycles_count / 1_000_000,
        format_units(min_price, "ether")?,
        format_units(mcycle_max_price, "ether")?
    );

    // Add to the max price an estimated upper bound on the gas costs.
    // Add a 10% buffer to the gas costs to account for flucuations after submission.
    let gas_price: u128 = boundless_client.provider().get_gas_price().await?;
    let gas_cost_estimate = gas_price + (gas_price / 10) * LOCK_FULFILL_GAS_UPPER_BOUND;
    let max_price = mcycle_max_price + U256::from(gas_cost_estimate);
    tracing::info!(
        "Setting a max price of {} ether: {} mcycle_price + {} gas_cost_estimate",
        format_units(max_price, "ether")?,
        format_units(mcycle_max_price, "ether")?,
        format_units(gas_cost_estimate, "ether")?,
    );

    let journal = session_info.journal;

    let request = ProofRequest::builder()
        .with_image_url(params.image_url)
        .with_input(input_url)
        .with_requirements(Requirements::new(
            ZETH_GUESTS_RETH_ETHEREUM_ID,
            Predicate::digest_match(journal.digest()),
        ))
        .with_offer(
            Offer::default()
                .with_min_price(min_price)
                .with_max_price(max_price)
                .with_ramp_up_period(params.ramp_up)
                .with_timeout(params.timeout)
                .with_lock_stake(params.stake)
                .with_lock_timeout(params.lock_timeout),
        )
        .build()?;

    // Send the request.
    let (request_id, _) = if params.offchain {
        boundless_client.submit_request_offchain(&request).await?
    } else {
        boundless_client.submit_request(&request).await?
    };

    tracing::info!(
        "Submitted request for block {} {} with id {}",
        build_args.block_number,
        if params.offchain { "offchain" } else { "onchain" },
        request_id
    );

    Ok(request_id)
}

async fn handle_failure(
    consecutive_failures: &mut u32,
    context: impl AsRef<str>,
    max_retries: u32,
) -> Result<()> {
    *consecutive_failures += 1;
    tracing::info!("(attempt {}/{}) {}", consecutive_failures, max_retries, context.as_ref());
    if *consecutive_failures >= max_retries {
        return Err(anyhow!("Operation failed after {} attempts", max_retries));
    }
    tokio::time::sleep(Duration::from_secs(RETRY_DELAY_SECS)).await;
    Ok(())
}
