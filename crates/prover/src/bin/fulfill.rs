// Copyright (c) 2024 RISC Zero, Inc.
//
// All rights reserved.
use std::time::Duration;

use alloy::{
    primitives::{Address, B256, U256},
    providers::{network::EthereumWallet, ProviderBuilder},
    signers::{local::PrivateKeySigner, Signature},
};
use anyhow::{bail, Result};
use boundless_market::{
    contracts::{boundless_market::BoundlessMarketService, set_verifier::SetVerifierService},
    order_stream_client::Order,
};
use clap::Parser;
use url::Url;

use boundless_prover::{fetch_url, DefaultProver};

#[derive(Parser, Debug)]
#[clap(author, version, about)]
enum Command {
    /// Fulfill a proof request using the RISC Zero zkVM default prover
    /// and submit it to the Boundless market.
    Submit {
        /// URL of the Ethereum RPC endpoint
        #[clap(short, long, env, default_value = "http://localhost:8545")]
        rpc_url: Url,
        /// Private key of the wallet
        #[clap(long, env)]
        private_key: PrivateKeySigner,
        /// Address of the market contract
        #[clap(short, long, env)]
        boundless_market_address: Address,
        /// Address of the SetVerifier contract
        #[clap(short, long, env)]
        set_verifier_address: Address,
        /// Tx timeout in seconds
        #[clap(long, env)]
        tx_timeout: Option<u64>,
        /// The proof request identifier
        #[clap(long)]
        request_id: U256,
        /// The tx hash of the request submission
        #[clap(long)]
        tx_hash: Option<B256>,
    },
    // Print {
    //     #[clap(long)]
    //     set_builder_url: String,
    //     #[clap(long)]
    //     assessor_url: String,
    // }
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

    run(Command::try_parse()?).await
}

pub(crate) async fn run(command: Command) -> Result<()> {
    match command {
        Command::Submit {
            rpc_url,
            private_key,
            boundless_market_address,
            set_verifier_address,
            tx_timeout,
            request_id,
            tx_hash,
        } => {
            let caller = private_key.address();
            let wallet = EthereumWallet::from(private_key.clone());
            let provider = ProviderBuilder::new()
                .with_recommended_fillers()
                .wallet(wallet)
                .on_http(rpc_url.clone());
            let mut boundless_market =
                BoundlessMarketService::new(boundless_market_address, provider.clone(), caller);
            if let Some(tx_timeout) = tx_timeout {
                boundless_market = boundless_market.with_timeout(Duration::from_secs(tx_timeout));
            }
            let (_, market_url) = boundless_market.image_info().await?;
            let assessor_elf = fetch_url(&market_url).await?;
            let domain = boundless_market.eip712_domain().await?;

            let mut set_verifier =
                SetVerifierService::new(set_verifier_address, provider.clone(), caller);
            if let Some(tx_timeout) = tx_timeout {
                set_verifier = set_verifier.with_timeout(Duration::from_secs(tx_timeout));
            }
            let (_, set_builder_url) = set_verifier.image_info().await?;
            let set_builder_elf = fetch_url(&set_builder_url).await?;

            let prover = DefaultProver::new(set_builder_elf, assessor_elf, caller, domain)?;

            let (request, sig) =
                boundless_market.get_submitted_request(request_id, tx_hash).await?;
            request.verify_signature(
                &sig,
                boundless_market_address,
                boundless_market.get_chain_id().await?,
            )?;
            let order = Order { request, signature: Signature::try_from(sig.as_ref())? };

            let order_fulfilled = prover.fulfill(order.clone(), false).await?;

            set_verifier.submit_merkle_root(order_fulfilled.root, order_fulfilled.seal).await?;

            match boundless_market
                .price_and_fulfill_batch(
                    vec![order.request],
                    vec![sig],
                    order_fulfilled.fills,
                    order_fulfilled.assessorSeal,
                    caller,
                    None,
                )
                .await
            {
                Ok(_) => {
                    tracing::info!("Fulfilled request 0x{:x}", request_id);
                }
                Err(e) => {
                    tracing::error!("Failed to fulfill request 0x{:x}: {}", request_id, e);
                }
            }

            Ok(())
        }
    }
}
