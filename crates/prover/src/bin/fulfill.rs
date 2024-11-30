// Copyright (c) 2024 RISC Zero, Inc.
//
// All rights reserved.

use std::{
    fs::{File, OpenOptions},
    io,
    io::Write,
    os::unix::io::{AsRawFd, FromRawFd},
    time::Duration,
};

use alloy::{
    hex::FromHex,
    primitives::{Address, Bytes, PrimitiveSignature, B256, U256},
    providers::{network::EthereumWallet, ProviderBuilder},
    signers::local::PrivateKeySigner,
    sol_types::SolValue,
};
use anyhow::{bail, ensure, Context, Result};
use boundless_market::{
    contracts::{
        boundless_market::BoundlessMarketService, eip712_domain, set_verifier::SetVerifierService,
        ProofRequest,
    },
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
    /// Print the result of fulfilling a proof request using the RISC Zero zkVM default prover.
    /// This is used to generate test FFI calls for the Forge cheatcode.
    Print {
        /// URL of the SetBuilder ELF
        #[clap(long)]
        set_builder_url: String,
        /// URL of the Assessor ELF
        #[clap(long)]
        assessor_url: String,
        /// Address of the prover
        #[clap(long)]
        prover_address: Address,
        /// Address of the Boundless market contract
        #[clap(long)]
        boundless_market_address: Address,
        /// Chain ID of the network where Boundless market contract is deployed
        #[clap(long)]
        chain_id: U256,
        /// Hex encoded proof request
        #[clap(long)]
        request: String,
        /// Hex encoded request' signature
        #[clap(long)]
        signature: String,
    },
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
            let order = Order { request, signature: PrimitiveSignature::try_from(sig.as_ref())? };

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
        Command::Print {
            set_builder_url,
            assessor_url,
            boundless_market_address,
            chain_id,
            prover_address,
            request,
            signature,
        } => {
            // Take stdout is ensure no extra data is written to it.
            let mut stdout = take_stdout()?;
            let set_builder_elf = fetch_url(&set_builder_url).await?;
            let assessor_elf = fetch_url(&assessor_url).await?;
            let domain = eip712_domain(boundless_market_address, chain_id.try_into()?);
            let prover = DefaultProver::new(set_builder_elf, assessor_elf, prover_address, domain)?;
            let order = Order {
                request: <ProofRequest>::abi_decode(
                    &hex::decode(request.trim_start_matches("0x"))?,
                    true,
                )
                .map_err(|_| anyhow::anyhow!("Failed to decode ProofRequest from input"))?,
                signature: PrimitiveSignature::try_from(
                    Bytes::from_hex(signature.trim_start_matches("0x"))?.as_ref(),
                )?,
            };
            let order_fulfilled = prover.fulfill(order, false).await?;

            // Forge test FFI calls expect hex encoded bytes sent to stdout
            write!(&mut stdout, "{}", hex::encode(order_fulfilled.abi_encode()))
                .context("failed to write to stdout")?;
            stdout.flush().context("failed to flush stdout")?;
            Ok(())
        }
    }
}

/// "Takes" stdout, returning a handle and ensuring no other code in this process can write to it.
/// This is used to ensure that no additional data (e.g. log lines) is written to stdout, as any
/// extra will cause a decoding failure in the Forge FFI cheatcode.
fn take_stdout() -> Result<File> {
    let stdout = io::stdout();
    let mut handle = stdout.lock();
    // Ensure all buffered data is written before redirection
    handle.flush()?;

    let devnull = OpenOptions::new().write(true).open("/dev/null")?;

    unsafe {
        // Create a copy of stdout to use for our output.
        let dup_fd = libc::dup(handle.as_raw_fd());
        ensure!(dup_fd >= 0, "call to libc::dup failed: {}", dup_fd);
        // Redirect stdout to the fd we opened for /dev/null
        let dup2_result = libc::dup2(devnull.as_raw_fd(), libc::STDOUT_FILENO);
        ensure!(dup2_result >= 0, "call to libc::dup2 failed: {}", dup2_result);
        Ok(File::from_raw_fd(dup_fd))
    }
}
