// Copyright (c) 2025 RISC Zero, Inc.
//
// All rights reserved.

use std::{
    path::PathBuf,
    time::{Duration, SystemTime},
};

use alloy::{
    primitives::{
        utils::{format_units, parse_ether},
        Address, U256,
    },
    signers::local::PrivateKeySigner,
};
use anyhow::{bail, Result};
use boundless_market::{
    client::ClientBuilder,
    contracts::{Input, Offer, Predicate, ProofRequest, Requirements},
    input::InputBuilder,
    storage::StorageProviderConfig,
};
use clap::{Args, Parser};
use risc0_zkvm::{compute_image_id, default_executor, sha::Digestible};
use url::Url;

/// Arguments of the order generator.
#[derive(Parser, Debug)]
#[clap(author, version, about, long_about = None)]
struct MainArgs {
    /// URL of the Ethereum RPC endpoint.
    #[clap(short, long, env)]
    rpc_url: Url,
    /// Optional URL of the offchain order stream endpoint.
    ///
    /// If set, the order-generator will submit requests off-chain.
    #[clap(short, long)]
    order_stream_url: Option<Url>,
    // Storage provider to use.
    #[clap(flatten)]
    storage_config: Option<StorageProviderConfig>,
    /// Private key used to sign and submit requests.
    #[clap(long, env)]
    private_key: PrivateKeySigner,
    /// Address of the SetVerifier contract.
    #[clap(short, long, env)]
    set_verifier_address: Address,
    /// Address of the BoundlessMarket contract.
    #[clap(short, long, env)]
    boundless_market_address: Address,
    /// Interval in seconds between requests.
    #[clap(short, long, default_value = "60")]
    interval: u64,
    /// Optional number of requests to submit.
    ///
    /// If unspecified, the loop will run indefinitely.
    #[clap(short, long)]
    count: Option<u64>,
    /// Minimum price per mcycle in ether.
    #[clap(long = "min", value_parser = parse_ether, default_value = "0.001")]
    min_price_per_mcycle: U256,
    /// Maximum price per mcycle in ether.
    #[clap(long = "max", value_parser = parse_ether, default_value = "0.002")]
    max_price_per_mcycle: U256,
    /// Lockin stake amount in ether.
    #[clap(short, long, value_parser = parse_ether, default_value = "0.0")]
    lockin_stake: U256,
    /// Number of blocks, from the current block, before the bid starts.
    #[clap(long, default_value = "5")]
    bidding_start_offset: u64,
    /// Ramp-up period in blocks.
    ///
    /// The bid price will increase linearly from `min_price` to `max_price` over this period.
    #[clap(long, default_value = "0")]
    ramp_up: u32,
    /// Number of blocks before the request lock-in expires.
    #[clap(long, default_value = "300")]
    lock_timeout: u32,
    /// Number of blocks before the request expires.
    #[clap(long, default_value = "300")]
    timeout: u32,
    /// Elf file to use as the guest image, given as a path.
    ///
    /// If unspecified, defaults to the included echo guest.
    #[clap(long)]
    elf: Option<PathBuf>,
    /// Input for the guest, given as a string or a path to a file.
    ///
    /// If unspecified, defaults to the current (risc0_zkvm::serde encoded) timestamp.
    #[command(flatten)]
    input: OrderInput,
    /// Use risc0_zkvm::serde to encode the input as a `Vec<u8>`
    #[clap(short, long)]
    encode_input: bool,
}

#[derive(Args, Clone, Debug)]
#[group(required = false, multiple = false)]
struct OrderInput {
    /// Input for the guest, given as a hex-encoded string.
    #[clap(long, value_parser = |s: &str| hex::decode(s))]
    input: Option<Vec<u8>>,
    /// Input for the guest, given as a path to a file.
    #[clap(long)]
    input_file: Option<PathBuf>,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    match dotenvy::dotenv() {
        Ok(path) => tracing::debug!("Loaded environment variables from {:?}", path),
        Err(e) if e.not_found() => tracing::debug!("No .env file found"),
        Err(e) => bail!("failed to load .env file: {}", e),
    }

    let args = MainArgs::parse();

    // NOTE: Using a separate `run` function to facilitate testing below.
    run(&args).await?;

    Ok(())
}

async fn run(args: &MainArgs) -> Result<()> {
    let boundless_client = ClientBuilder::default()
        .with_rpc_url(args.rpc_url.clone())
        .with_boundless_market_address(args.boundless_market_address)
        .with_set_verifier_address(args.set_verifier_address)
        .with_order_stream_url(args.order_stream_url.clone())
        .with_storage_provider_config(args.storage_config.clone())
        .with_private_key(args.private_key.clone())
        .with_bidding_start_offset(args.bidding_start_offset)
        .build()
        .await?;

    let elf = match &args.elf {
        Some(path) => std::fs::read(path)?,
        None => {
            // A build of the echo guest, which simply commits the bytes it reads from inputs.
            let url = "https://gateway.pinata.cloud/ipfs/bafkreihfm2xxqdh336jhcrg6pfrigsfzrqgxyzilhq5rju66gyebrjznpy";
            fetch_http(&Url::parse(url)?).await?
        }
    };
    let image_id = compute_image_id(&elf)?;

    let image_url = boundless_client.upload_image(&elf).await?;
    tracing::info!("Uploaded image to {}", image_url);

    let mut i = 0u64;
    loop {
        if let Some(count) = args.count {
            if i >= count {
                break;
            }
        }

        let input: Vec<u8> = match (args.input.input.clone(), args.input.input_file.clone()) {
            (Some(input), None) => input,
            (None, Some(input_file)) => std::fs::read(input_file)?,
            (None, None) => format! {"{:?}", SystemTime::now()}.as_bytes().to_vec(),
            _ => bail!("at most one of input or input-file args must be provided"),
        };

        let env = if args.encode_input {
            InputBuilder::new().write(&input)?.build_env()?
        } else {
            InputBuilder::new().write_slice(&input).build_env()?
        };

        let session_info = default_executor().execute(env.clone().try_into()?, &elf)?;
        let journal = session_info.journal;

        let cycles_count =
            session_info.segments.iter().map(|segment| 1 << segment.po2).sum::<u64>();
        let min_price = args
            .min_price_per_mcycle
            .checked_mul(U256::from(cycles_count))
            .unwrap()
            .div_ceil(U256::from(1_000_000));
        let max_price = args
            .max_price_per_mcycle
            .checked_mul(U256::from(cycles_count))
            .unwrap()
            .div_ceil(U256::from(1_000_000));

        tracing::info!(
            "{} cycles count {} min_price in ether {} max_price in ether",
            cycles_count,
            format_units(min_price, "ether")?,
            format_units(max_price, "ether")?
        );

        let request = ProofRequest::builder()
            .with_image_url(image_url.clone())
            .with_input(Input::inline(env.encode()?))
            .with_requirements(Requirements::new(
                image_id,
                Predicate::digest_match(journal.digest()),
            ))
            .with_offer(
                Offer::default()
                    .with_min_price(min_price)
                    .with_max_price(max_price)
                    .with_lock_stake(args.lockin_stake)
                    .with_ramp_up_period(args.ramp_up)
                    .with_timeout(args.timeout)
                    .with_lock_timeout(args.lock_timeout),
            )
            .build()?;

        tracing::info!("Request: {:?}", request);

        let submit_offchain = args.order_stream_url.is_some();
        let (request_id, _) = if submit_offchain {
            boundless_client.submit_request_offchain(&request).await?
        } else {
            boundless_client.submit_request(&request).await?
        };

        if submit_offchain {
            tracing::info!("Request 0x{request_id:x} submitted offchain");
        } else {
            tracing::info!("Request 0x{request_id:x} submitted onchain");
        }

        i += 1;
        tokio::time::sleep(Duration::from_secs(args.interval)).await;
    }

    Ok(())
}

async fn fetch_http(url: &Url) -> Result<Vec<u8>> {
    let response = reqwest::get(url.as_str()).await?;
    let status = response.status();
    if !status.is_success() {
        bail!("HTTP request failed with status: {}", status);
    }

    Ok(response.bytes().await?.to_vec())
}

#[cfg(test)]
mod tests {
    use alloy::{
        node_bindings::Anvil, providers::Provider, rpc::types::Filter, sol_types::SolEvent,
    };
    use boundless_market::contracts::{test_utils::DefaultTestCtx, IBoundlessMarket};
    use guest_assessor::ASSESSOR_GUEST_ID;
    use guest_set_builder::SET_BUILDER_ID;
    use tracing_test::traced_test;

    use super::*;

    #[tokio::test]
    #[traced_test]
    async fn test_main() {
        let anvil = Anvil::new().spawn();
        let ctx = DefaultTestCtx::create(&anvil, SET_BUILDER_ID, ASSESSOR_GUEST_ID).await.unwrap();

        let args = MainArgs {
            rpc_url: anvil.endpoint_url(),
            order_stream_url: None,
            storage_config: Some(StorageProviderConfig::dev_mode()),
            private_key: ctx.customer_signer,
            set_verifier_address: ctx.set_verifier_addr,
            boundless_market_address: ctx.boundless_market_addr,
            interval: 1,
            count: Some(2),
            min_price_per_mcycle: parse_ether("0.001").unwrap(),
            max_price_per_mcycle: parse_ether("0.002").unwrap(),
            lockin_stake: parse_ether("0.0").unwrap(),
            bidding_start_offset: 5,
            ramp_up: 0,
            timeout: 1000,
            lock_timeout: 1000,
            elf: None,
            input: OrderInput { input: None, input_file: None },
            encode_input: false,
        };

        run(&args).await.unwrap();

        // Check that the requests were submitted
        let filter = Filter::new()
            .event_signature(IBoundlessMarket::RequestSubmitted::SIGNATURE_HASH)
            .from_block(0)
            .address(ctx.boundless_market_addr);
        let logs = ctx.customer_provider.get_logs(&filter).await.unwrap();
        let decoded_logs = logs.iter().filter_map(|log| {
            match log.log_decode::<IBoundlessMarket::RequestSubmitted>() {
                Ok(res) => Some(res),
                Err(err) => {
                    tracing::error!("Failed to decode RequestSubmitted log: {err:?}");
                    None
                }
            }
        });
        assert!(decoded_logs.count() == 2);
    }
}
