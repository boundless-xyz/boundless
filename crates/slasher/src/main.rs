// Copyright (c) 2025 RISC Zero, Inc.
//
// All rights reserved.

use std::{path::PathBuf, time::Duration};

use alloy::{primitives::Address, signers::local::PrivateKeySigner};
use anyhow::{bail, Result};
use boundless_slasher::SlashService;
use clap::{Args, Parser};
use url::Url;

/// Arguments of the order generator.
#[derive(Parser, Debug)]
#[clap(author, version, about, long_about = None)]
struct MainArgs {
    /// URL of the Ethereum RPC endpoint.
    #[clap(short, long, env)]
    rpc_url: Url,
    /// Private key used to sign and submit slash requests.
    #[clap(long, env)]
    private_key: PrivateKeySigner,
    /// Address of the BoundlessMarket contract.
    #[clap(short, long, env)]
    boundless_market_address: Address,
    /// DB connection string.
    #[clap(short, long, default_value = "sqlite::memory:")]
    db: String,
    /// Starting block number.
    #[clap(long)]
    start_block: Option<u64>,
    /// Interval in seconds between checking for expired requests.
    #[clap(short, long, default_value = "60")]
    interval: u64,
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
    let slash_service = SlashService::new(
        args.rpc_url.clone(),
        &args.private_key,
        args.boundless_market_address,
        &args.db,
        Duration::from_secs(args.interval),
    )
    .await?;

    if let Err(err) = slash_service.run(args.start_block).await {
        bail!("Error running the slasher: {err}");
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use alloy::{
        node_bindings::Anvil,
        primitives::{Bytes, B256, U256},
        providers::Provider,
        signers::Signer,
    };
    use boundless_market::contracts::{
        test_utils::TestCtx, Input, Offer, Predicate, PredicateType, ProofRequest, Requirements,
    };
    use futures_util::StreamExt;
    use guest_assessor::ASSESSOR_GUEST_ID;
    use guest_set_builder::SET_BUILDER_ID;
    use risc0_zkvm::sha::Digest;

    use super::*;

    async fn create_order(
        signer: &impl Signer,
        signer_addr: Address,
        order_id: u32,
        contract_addr: Address,
        chain_id: u64,
        current_block: u64,
    ) -> (ProofRequest, Bytes) {
        let req = ProofRequest::new(
            order_id,
            &signer_addr,
            Requirements {
                imageId: B256::ZERO,
                predicate: Predicate {
                    predicateType: PredicateType::PrefixMatch,
                    data: Default::default(),
                },
            },
            "https://dev.null".to_string(),
            Input::builder().build_inline().unwrap(),
            Offer {
                minPrice: U256::from(0),
                maxPrice: U256::from(1),
                biddingStart: 0,
                timeout: current_block as u32 + 2,
                rampUpPeriod: 1,
                lockStake: U256::from(0),
            },
        );

        let client_sig = req.sign_request(signer, contract_addr, chain_id).await.unwrap();

        (req, client_sig.as_bytes().into())
    }

    #[tokio::test]
    async fn test_main() {
        let anvil = Anvil::new().spawn();
        let ctx =
            TestCtx::new(&anvil, Digest::from(SET_BUILDER_ID), Digest::from(ASSESSOR_GUEST_ID))
                .await
                .unwrap();

        let args = MainArgs {
            rpc_url: anvil.endpoint_url(),
            private_key: ctx.customer_signer.clone(),
            boundless_market_address: ctx.boundless_market_addr,
            db: "sqlite::memory:".to_string(),
            start_block: None,
            interval: 1,
        };

        let main_handle = tokio::spawn(async move { run(&args).await });

        // Subscribe to slash events before operations
        let slash_event =
            ctx.customer_market.instance().ProverSlashed_filter().watch().await.unwrap();
        let mut stream = slash_event.into_stream();
        println!("Subscribed to ProverSlashed event");

        let (request, client_sig) = create_order(
            &ctx.customer_signer,
            ctx.customer_signer.address(),
            1,
            ctx.boundless_market_addr,
            anvil.chain_id(),
            ctx.customer_provider.get_block_number().await.unwrap(),
        )
        .await;

        // Do the operations that should trigger the slash
        ctx.customer_market.deposit(U256::from(1)).await.unwrap();
        ctx.prover_market.lock_request(&request, &client_sig, None).await.unwrap();

        // Wait for the slash event with timeout
        tokio::select! {
            Some(event) = stream.next() => {
                let request_slashed = event.unwrap().0;
                println!("Detected prover slashed for request {:?}", request_slashed.requestId);
                assert_eq!(request_slashed.prover, ctx.prover_signer.address());
                main_handle.abort();
            }
            _ = tokio::time::sleep(Duration::from_secs(10)) => {
                panic!("Test timed out waiting for slash event");
            }
        }

        let _ = main_handle.await;
    }
}
