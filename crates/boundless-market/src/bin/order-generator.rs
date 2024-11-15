// Copyright (c) 2024 RISC Zero, Inc.
//
// All rights reserved.

use std::{
    path::PathBuf,
    time::{Duration, SystemTime},
};

use alloy::{
    primitives::{aliases::U96, utils::parse_ether, Address},
    signers::local::PrivateKeySigner,
};
use anyhow::{bail, Result};
use boundless_market::{
    client::Client,
    contracts::{Input, Offer, Predicate, ProvingRequest, Requirements},
    storage::{storage_provider_from_env, BuiltinStorageProvider, TempFileStorageProvider},
};
use clap::{Args, Parser};
use guest_util::{ECHO_ELF, ECHO_ID};
use risc0_zkvm::{compute_image_id, default_executor, sha::Digestible, ExecutorEnv};
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
    /// Private key used to sign and submit requests.
    #[clap(long, env)]
    private_key: PrivateKeySigner,
    /// Address of the SetVerifier contract.
    #[clap(short, long, env)]
    set_verifier_address: Address,
    /// Address of the ProofMarket contract.
    #[clap(short, long, env)]
    proof_market_address: Address,
    /// Interval in seconds between requests.
    #[clap(short, long, default_value = "60")]
    interval: u64,
    /// Optional number of requests to submit.
    ///
    /// If unspecified, the loop will run indefinitely.
    #[clap(short, long)]
    count: Option<u64>,
    /// Minimum price per mcycle in ether.
    #[clap(long = "min", default_value = "0.001")]
    min_price_per_mcycle: String,
    /// Maximum price per mcycle in ether.
    #[clap(long = "max", default_value = "0.002")]
    max_price_per_mcycle: String,
    /// Lockin stake amount in ether.
    #[clap(short, long, default_value = "0.0")]
    lockin_stake: String,
    /// Number of blocks before the request expires.
    #[clap(long, default_value = "1000")]
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
    /// Input for the guest, given as a string.
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
    let storage_provider = if cfg!(test) {
        BuiltinStorageProvider::File(TempFileStorageProvider::new().await?)
    } else {
        storage_provider_from_env().await?
    };
    let order_stream_url =
        args.order_stream_url.clone().unwrap_or(url::Url::parse("http://localhost:8585")?);
    let boundless_client = Client::from_parts(
        args.private_key.clone(),
        args.rpc_url.clone(),
        args.proof_market_address,
        args.set_verifier_address,
        order_stream_url,
        storage_provider,
    )
    .await?;

    let (elf, image_id) = if let Some(elf) = args.elf.clone() {
        let elf = std::fs::read(elf)?;
        (elf.clone(), <[u32; 8]>::from(compute_image_id(&elf)?))
    } else {
        (ECHO_ELF.to_vec(), ECHO_ID)
    };

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
        let encoded_input = if args.encode_input
            || (args.input.input.is_none() && args.input.input_file.is_none())
        {
            bytemuck::pod_collect_to_vec(&risc0_zkvm::serde::to_vec(&input)?)
        } else {
            input
        };

        let env = ExecutorEnv::builder().write_slice(&encoded_input).build()?;
        let session_info = default_executor().execute(env, &elf)?;
        let mcycles_count = session_info
            .segments
            .iter()
            .map(|segment| 1 << segment.po2)
            .sum::<u64>()
            .div_ceil(1_000_000);
        let journal = session_info.journal;

        let request = ProvingRequest::default()
            .with_image_url(&image_url)
            .with_input(Input::inline(encoded_input))
            .with_requirements(Requirements::new(
                image_id,
                Predicate::digest_match(journal.digest()),
            ))
            .with_offer(
                Offer::default()
                    .with_min_price_per_mcycle(
                        U96::from::<u128>(parse_ether(&args.min_price_per_mcycle)?.try_into()?),
                        mcycles_count,
                    )
                    .with_max_price_per_mcycle(
                        U96::from::<u128>(parse_ether(&args.max_price_per_mcycle)?.try_into()?),
                        mcycles_count,
                    )
                    .with_lockin_stake(U96::from::<u128>(
                        parse_ether(&args.lockin_stake)?.try_into()?,
                    ))
                    .with_timeout(args.timeout),
            );

        let request_id = if let Some(_) = args.order_stream_url {
            boundless_client.submit_request_offchain(&request).await?
        } else {
            boundless_client.submit_request(&request).await?
        };

        tracing::info!("Request 0x{request_id:x} submitted");

        i += 1;
        tokio::time::sleep(Duration::from_secs(args.interval)).await;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use alloy::{
        node_bindings::Anvil, providers::Provider, rpc::types::Filter, sol_types::SolEvent,
    };
    use boundless_market::contracts::{test_utils::TestCtx, IProofMarket};
    use tracing_test::traced_test;

    use super::*;

    #[tokio::test]
    #[traced_test]
    async fn test_main() {
        let anvil = Anvil::new().spawn();
        let ctx = TestCtx::new(&anvil).await.unwrap();

        let args = MainArgs {
            rpc_url: anvil.endpoint_url(),
            order_stream_url: None,
            private_key: ctx.customer_signer,
            set_verifier_address: ctx.set_verifier_addr,
            proof_market_address: ctx.proof_market_addr,
            interval: 1,
            count: Some(2),
            min_price_per_mcycle: "0.001".into(),
            max_price_per_mcycle: "0.002".into(),
            lockin_stake: "0.0".into(),
            timeout: 1000,
            elf: None,
            input: OrderInput { input: None, input_file: None },
            encode_input: false,
        };

        run(&args).await.unwrap();

        // Check that the requests were submitted
        let filter = Filter::new()
            .event_signature(IProofMarket::RequestSubmitted::SIGNATURE_HASH)
            .from_block(0)
            .address(ctx.proof_market_addr);
        let logs = ctx.customer_provider.get_logs(&filter).await.unwrap();
        let decoded_logs = logs.iter().filter_map(|log| {
            match log.log_decode::<IProofMarket::RequestSubmitted>() {
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
