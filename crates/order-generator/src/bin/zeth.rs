// Copyright (c) 2025 RISC Zero, Inc.
//
// All rights reserved.

use std::cmp::min;

use alloy::{
    network::{Ethereum, EthereumWallet},
    primitives::{utils::parse_ether, Address, U256},
    providers::{Provider, ProviderBuilder},
    signers::local::PrivateKeySigner,
    transports::Transport,
};
use anyhow::{anyhow, bail, Result};
use boundless_market::{
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

const MAX_RETRY_ATTEMPTS: u32 = 5;
const RETRY_DELAY_SECS: u64 = 5;

/// Arguments of the publisher CLI.
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
    /// Private key used to interact with the Counter contract.
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
    #[clap(long = "min", value_parser = parse_ether, default_value = "0.001")]
    min_price_per_mcycle: U256,
    /// Maximum price per mcycle in ether.
    #[clap(long = "max", value_parser = parse_ether, default_value = "0.0011")]
    max_price_per_mcycle: U256,
    /// Number of blocks, from the current block, before the bid expires.
    #[clap(long, default_value = "1000")]
    timeout: u32,
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
    let provider = ProviderBuilder::new()
        .with_recommended_fillers()
        .wallet(wallet)
        .on_http(args.zeth_rpc_url.clone());
    let rpc = Some(args.zeth_rpc_url.to_string());
    let chain_id = provider.get_chain_id().await?;
    let chain = Some(NamedChain::try_from(chain_id).map_err(|_| anyhow!("Unknown chain"))?);

    let boundless_client = ClientBuilder::default()
        .with_rpc_url(args.rpc_url)
        .with_boundless_market_address(args.boundless_market_address)
        .with_set_verifier_address(args.set_verifier_address)
        .with_order_stream_url(args.order_stream_url)
        .with_storage_provider_config(args.storage_config.clone())
        .with_private_key(args.private_key)
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
            Ok(number) => {
                consecutive_failures = 0; // Reset failures on success.
                number
            }
            Err(err) => {
                if let Err(e) = handle_failure(
                    &mut consecutive_failures,
                    format!("Failed to get block number: {}", err),
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
            if let Err(e) =
                handle_failure(&mut consecutive_failures, "Current block is behind expected block")
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

        // Attempt to submit a request.
        match submit_request(
            build_args,
            chain_id,
            boundless_client.clone(),
            image_url.clone(),
            args.min_price_per_mcycle,
            args.max_price_per_mcycle,
            args.timeout,
        )
        .await
        {
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

async fn submit_request<T, P, S>(
    build_args: BuildArgs,
    chain_id: u64,
    boundless_client: Client<T, P, S>,
    image_url: Url,
    min: U256,
    max: U256,
    timeout: u32,
) -> Result<U256>
where
    T: Transport + Clone,
    P: Provider<T, Ethereum> + 'static + Clone,
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
    let mcycles_count = session_info
        .segments
        .iter()
        .map(|segment| 1 << segment.po2)
        .sum::<u64>()
        .div_ceil(1_000_000);
    tracing::info!("{} mcycles count.", mcycles_count);
    let journal = session_info.journal;

    let request = ProofRequest::builder()
        .with_image_url(image_url)
        .with_input(input_url)
        .with_requirements(Requirements::new(
            ZETH_GUESTS_RETH_ETHEREUM_ID,
            Predicate::digest_match(journal.digest()),
        ))
        .with_offer(
            Offer::default()
                .with_min_price_per_mcycle(min, mcycles_count)
                .with_max_price_per_mcycle(max, mcycles_count)
                .with_timeout(timeout),
        )
        .build()?;

    // Send the request.
    let (request_id, _) = boundless_client.submit_request(&request).await?;

    Ok(request_id)
}

async fn handle_failure(consecutive_failures: &mut u32, context: impl AsRef<str>) -> Result<()> {
    *consecutive_failures += 1;
    tracing::warn!(
        "{} (attempt {}/{})",
        context.as_ref(),
        consecutive_failures,
        MAX_RETRY_ATTEMPTS
    );
    if *consecutive_failures >= MAX_RETRY_ATTEMPTS {
        return Err(anyhow!("Operation failed after {} attempts", MAX_RETRY_ATTEMPTS));
    }
    tokio::time::sleep(Duration::from_secs(RETRY_DELAY_SECS)).await;
    Ok(())
}
