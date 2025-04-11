// Copyright (c) 2025 RISC Zero, Inc.
//
// All rights reserved.

use std::path::PathBuf;

use alloy::{
    primitives::{Address, FixedBytes, U256},
    signers::local::PrivateKeySigner,
};
use anyhow::{bail, Result};
use boundless_market::{
    benchmark_directive,
    client::ClientBuilder,
    contracts::{Input, Offer, Predicate, ProofRequest, Requirements, Selector},
    input::InputBuilder,
    storage::{
        storage_provider_from_config, storage_provider_from_env, BuiltinStorageProvider,
        StorageProviderConfig,
    },
};
use clap::Parser;
use risc0_zkvm::{compute_image_id, default_executor};
use url::Url;

/// A CLI tool for dispatching benchmark requests to specific nodes in Boundless.
///
/// Benchmark requests require a pre-agreement between the order generator and the node being benchmarked
/// during which they must produce a shared secret. Benchmark requests use a special URI encoding for the input
/// which includes the hash of this secret so the broker can identify when it is being asked to perform a benchmark.
///
/// It also instructs the prover to prepend the benchmark secret to the input of the guest.
/// It is expect that any benchmarking guest code will hash this secret and commit to the hash in the journal.
/// This is used to ensure that only provers in possession of the secret can successfully fill the request.
#[derive(Parser, Debug)]
#[clap(author, version, about, long_about = None)]
struct Args {
    /// URL of the Ethereum RPC endpoint.
    #[clap(short, long, env)]
    rpc_url: Url,
    /// Optional URL of the offchain order stream endpoint.
    ///
    /// If set, the order-generator will submit requests off-chain.
    #[clap(short, long, env)]
    order_stream_url: Option<Url>,
    /// Private key used to sign and submit requests.
    #[clap(long, env)]
    private_key: PrivateKeySigner,
    /// Address of the SetVerifier contract.
    #[clap(short, long, env)]
    set_verifier_address: Address,
    /// Address of the BoundlessMarket contract.
    #[clap(short, long, env)]
    boundless_market_address: Address,
    // Storage provider to use.
    #[clap(flatten)]
    storage_config: Option<StorageProviderConfig>,
    /// Number of seconds before the request expires.
    #[clap(long, default_value = "1800")]
    timeout: u32,
    /// Elf file to use as the guest image, given as a path.
    #[clap(long)]
    elf: PathBuf,
    /// Input for the guest, given as a string or a path to a file.
    #[command(flatten)]
    input: OrderInput,
    /// Use risc0_zkvm::serde to encode the input as a `Vec<u8>`
    #[clap(short, long)]
    encode_input: bool,
    /// Node benchmark secret
    ///
    /// The hash of this is used encoded in the URL of the input to let the node know it is being asked to perform a benchmark
    /// The secret should be known only to the node and the order generator
    #[clap(long, value_parser = |s: &str| hex::decode(s))]
    benchmark_secret: Vec<u8>,
}

#[derive(clap::Args, Clone, Debug)]
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

    let args = Args::parse();

    run(&args).await?;

    Ok(())
}

async fn run(args: &Args) -> Result<()> {
    let storage_provider = match &args.storage_config {
        Some(storage_config) => storage_provider_from_config(storage_config).await?,
        None => storage_provider_from_env().await?,
    };

    let boundless_client = ClientBuilder::<BuiltinStorageProvider>::new()
        .with_rpc_url(args.rpc_url.clone())
        .with_storage_provider(Some(storage_provider))
        .with_boundless_market_address(args.boundless_market_address)
        .with_set_verifier_address(args.set_verifier_address)
        .with_order_stream_url(args.order_stream_url.clone())
        .with_private_key(args.private_key.clone())
        .build()
        .await?;

    let elf = std::fs::read(&args.elf)?;
    let image_id = compute_image_id(&elf)?;

    let image_url = boundless_client.upload_image(&elf).await?;
    tracing::info!("Uploaded image to {}", image_url);

    let input: Vec<u8> = match (args.input.input.clone(), args.input.input_file.clone()) {
        (Some(input), None) => input,
        (None, Some(input_file)) => std::fs::read(input_file)?,
        _ => bail!("exactly one of input or input-file args must be provided"),
    };
    let input = [args.benchmark_secret.clone(), input].concat();
    let mut input_url = boundless_client.upload_input(&input).await?;
    // Set the URL fragment for the benchmark directive for the node
    input_url.set_fragment(Some(&benchmark_directive(&args.benchmark_secret)));

    let env = if args.encode_input {
        InputBuilder::new().write(&input)?.build_env()?
    } else {
        InputBuilder::new().write_slice(&input).build_env()?
    };

    let session_info = default_executor().execute(env.clone().try_into()?, &elf)?;
    let journal = session_info.journal;
    let cycles_count = session_info.segments.iter().map(|segment| 1 << segment.po2).sum::<u64>();

    tracing::info!("{} cycles", cycles_count,);

    let request = ProofRequest::builder()
        .with_image_url(image_url.clone())
        .with_input(Input::url(input_url))
        .with_requirements(
            Requirements::new(image_id, Predicate::prefix_match(journal.bytes))
                .with_selector(FixedBytes::from(Selector::Groth16V2_0 as u32)),
        )
        .with_offer(
            Offer::default()
                .with_min_price(U256::from(1))
                .with_max_price(U256::from(1))
                .with_bidding_start(now_timestamp())
                .with_timeout(args.timeout)
                .with_lock_timeout(args.timeout),
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
        tracing::info!(
            "Request 0x{request_id:x} submitted offchain to {}",
            args.order_stream_url.clone().unwrap()
        );
    } else {
        tracing::info!(
            "Request 0x{request_id:x} submitted onchain to {}",
            args.boundless_market_address
        );
    }

    Ok(())
}

fn now_timestamp() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use alloy::{
        node_bindings::Anvil, providers::Provider, rpc::types::Filter, sol_types::SolEvent,
    };
    use boundless_market::{
        contracts::{test_utils::create_test_ctx, IBoundlessMarket},
        storage::StorageProviderConfig,
    };
    use guest_assessor::{ASSESSOR_GUEST_ID, ASSESSOR_GUEST_PATH};
    use guest_set_builder::{SET_BUILDER_ID, SET_BUILDER_PATH};
    use guest_util::ECHO_PATH;
    use tracing_test::traced_test;

    use super::*;

    #[tokio::test]
    #[traced_test]
    async fn test_main() {
        let anvil = Anvil::new().spawn();
        let ctx = create_test_ctx(
            &anvil,
            SET_BUILDER_ID,
            format!("file://{SET_BUILDER_PATH}"),
            ASSESSOR_GUEST_ID,
            format!("file://{ASSESSOR_GUEST_PATH}"),
        )
        .await
        .unwrap();

        let args = Args {
            rpc_url: anvil.endpoint_url(),
            order_stream_url: None,
            storage_config: Some(StorageProviderConfig::dev_mode()),
            private_key: ctx.customer_signer,
            set_verifier_address: ctx.set_verifier_address,
            boundless_market_address: ctx.boundless_market_address,
            timeout: 1000,
            elf: ECHO_PATH.into(),
            input: OrderInput { input: Some(b"echo this".into()), input_file: None },
            encode_input: false,
            benchmark_secret: vec![],
        };

        run(&args).await.unwrap();

        // Check that the requests were submitted
        let filter = Filter::new()
            .event_signature(IBoundlessMarket::RequestSubmitted::SIGNATURE_HASH)
            .from_block(0)
            .address(ctx.boundless_market_address);
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
