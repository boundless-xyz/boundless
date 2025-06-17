// Copyright 2025 RISC Zero, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use std::{
    str::FromStr,
    time::{Duration, SystemTime},
};

use crate::ICounter::ICounterInstance;
use alloy::{
    primitives::{Address, B256},
    signers::local::PrivateKeySigner,
    sol_types::SolCall,
};
use anyhow::{bail, Context, Result};
use boundless_market::{
    input::GuestEnv, request_builder::OfferParams, Client, Deployment, StorageProviderConfig,
};
use clap::Parser;
use guest_util::{ECHO_ELF, ECHO_ID, IDENTITY_ELF, IDENTITY_ID};
use risc0_ethereum_contracts::receipt::Receipt as ContractReceipt;
use risc0_zkvm::sha::{Digest, Digestible};
use tracing_subscriber::{filter::LevelFilter, prelude::*, EnvFilter};
use url::Url;

/// Timeout for the transaction to be confirmed.
pub const TX_TIMEOUT: Duration = Duration::from_secs(30);

alloy::sol!(
    #![sol(rpc, all_derives)]
    "../contracts/src/ICounter.sol"
);

/// CLI arguments.
#[derive(Parser, Debug)]
#[clap(author, version, about, long_about = None)]
struct Args {
    /// URL of the Ethereum RPC endpoint.
    #[clap(short, long, env)]
    rpc_url: Url,
    /// Private key used to interact with the Counter contract and the Boundless Market.
    #[clap(long, env)]
    private_key: PrivateKeySigner,
    /// Address of the Counter contract.
    #[clap(short, long, env)]
    counter_address: Address,
    /// Configuration for the StorageProvider to use for uploading programs and inputs.
    #[clap(flatten, next_help_heading = "Storage Provider")]
    storage_config: StorageProviderConfig,
    /// Boundless Market deployment configuration
    #[clap(flatten, next_help_heading = "Boundless Market Deployment")]
    deployment: Option<Deployment>,
}

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize logging.
    tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer())
        .with(
            EnvFilter::builder()
                .with_default_directive(LevelFilter::from_str("info")?.into())
                .from_env_lossy(),
        )
        .init();

    let args = Args::parse();

    // NOTE: Using a separate `run` function to facilitate testing.
    run(args).await
}

/// Main logic which creates the Boundless client, executes the proofs and submits the tx.
async fn run(args: Args) -> Result<()> {
    // Create a Boundless client from the provided parameters.
    let client = Client::builder()
        .with_rpc_url(args.rpc_url)
        .with_deployment(args.deployment)
        .with_storage_provider_config(&args.storage_config)?
        .with_private_key(args.private_key)
        .build()
        .await
        .context("failed to build boundless client")?;

    // Request an un-aggregated proof from the Boundless market using the ECHO guest.
    let echo_request = client
        .new_request()
        .with_program(ECHO_ELF)
        .with_stdin(format!("{:?}", SystemTime::now()).as_bytes())
        .with_groth16_proof();

    // Submit the request to the Boundless market
    let (request_id, expires_at) = client.submit_onchain(echo_request).await?;
    tracing::info!("Request {:x} submitted", request_id);

    // Wait for the request to be fulfilled (check periodically)
    tracing::info!("Waiting for request {:x} to be fulfilled", request_id);
    let (echo_journal, echo_seal) = client
        .wait_for_request_fulfillment(
            request_id,
            Duration::from_secs(5), // periodic check every 5 seconds
            expires_at,
        )
        .await?;
    tracing::info!("Request {:x} fulfilled", request_id);

    // Decode the resulting RISC0-ZKVM receipt.
    let Ok(ContractReceipt::Base(echo_receipt)) =
        risc0_ethereum_contracts::receipt::decode_seal(echo_seal, ECHO_ID, echo_journal.clone())
    else {
        bail!("did not receive requested unaggregated receipt")
    };
    let echo_claim_digest = echo_receipt.claim().unwrap().digest();

    // Build the IDENTITY input with from the ECHO receipt.
    let identity_input = (Digest::from(ECHO_ID), echo_receipt);
    let identity_request = client
        .new_request()
        .with_program(IDENTITY_ELF)
        // Set lock timeout to 20 minutes to allow this example to be run onn slower provers.
        .with_offer(OfferParams::builder().lock_timeout(1200).timeout(1200))
        .with_env(GuestEnv::builder().write_frame(&postcard::to_allocvec(&identity_input)?));

    // Submit the request to the Boundless market
    let (request_id, expires_at) = client.submit_onchain(identity_request).await?;
    tracing::info!("Request {:x} submitted", request_id);

    // Wait for the request to be fulfilled (check periodically)
    tracing::info!("Waiting for request {:x} to be fulfilled", request_id);
    let (identity_journal, identity_seal) = client
        .wait_for_request_fulfillment(
            request_id,
            Duration::from_secs(5), // periodic check every 5 seconds
            expires_at,
        )
        .await?;
    tracing::info!("Request {:x} fulfilled", request_id);
    debug_assert_eq!(&identity_journal, echo_claim_digest.as_bytes());

    // Interact with the Counter contract by calling the increment function.
    let counter = ICounterInstance::new(args.counter_address, client.provider());
    let journal_digest = B256::from_slice(identity_journal.digest().as_bytes());
    let image_id = B256::from_slice(Digest::from(IDENTITY_ID).as_bytes());
    let call_increment =
        counter.increment(identity_seal, image_id, journal_digest).from(client.caller());

    tracing::info!("Calling Counter increment function");
    let pending_tx = call_increment.send().await.context("failed to broadcast transaction")?;
    tracing::info!("Broadcasting tx {}", pending_tx.tx_hash());
    let tx_hash = pending_tx
        .with_timeout(Some(TX_TIMEOUT))
        .watch()
        .await
        .context("failed to confirm transaction")?;
    tracing::info!("Tx {:?} confirmed", tx_hash);

    // Query the counter value for the caller address.
    let count = counter
        .getCount(client.caller())
        .call()
        .await
        .with_context(|| format!("failed to call {}", ICounter::getCountCall::SIGNATURE))?;
    tracing::info!("Counter value for address {:?} is {:?}", client.caller(), count);

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::{
        network::EthereumWallet,
        node_bindings::{Anvil, AnvilInstance},
        providers::{Provider, ProviderBuilder, WalletProvider},
    };
    use boundless_market::{
        contracts::hit_points::default_allowance, storage::StorageProviderType,
    };
    use boundless_market_test_utils::{create_test_ctx, TestCtx};
    use broker::test_utils::BrokerBuilder;
    use test_log::test;
    use tokio::task::JoinSet;

    alloy::sol!(
        #![sol(rpc)]
        Counter,
        "../contracts/out/Counter.sol/Counter.json"
    );

    async fn deploy_counter<P: Provider + 'static + Clone + WalletProvider>(
        anvil: &AnvilInstance,
        test_ctx: &TestCtx<P>,
    ) -> Result<Address> {
        let deployer_signer: PrivateKeySigner = anvil.keys()[0].clone().into();
        let deployer_provider = ProviderBuilder::new()
            .wallet(EthereumWallet::from(deployer_signer))
            .connect(&anvil.endpoint())
            .await
            .unwrap();
        let counter = Counter::deploy(
            &deployer_provider,
            test_ctx
                .deployment
                .verifier_router_address
                .context("deployment is missing verifier_router_address")?,
        )
        .await?;
        Ok(*counter.address())
    }

    #[test(tokio::test)]
    async fn test_main() -> Result<()> {
        // Setup anvil and deploy contracts.
        let anvil = Anvil::new().spawn();
        let ctx = create_test_ctx(&anvil).await.unwrap();
        ctx.prover_market
            .deposit_stake_with_permit(default_allowance(), &ctx.prover_signer)
            .await
            .unwrap();
        let counter_address = deploy_counter(&anvil, &ctx).await.unwrap();

        // A JoinSet automatically aborts all its tasks when dropped
        let mut tasks = JoinSet::new();

        // Start a broker.
        let (broker, _config) =
            BrokerBuilder::new_test(&ctx, anvil.endpoint_url()).await.build().await?;
        tasks.spawn(async move { broker.start_service().await });

        const TIMEOUT_SECS: u64 = 1800; // 30 minutes

        let run_task = run(Args {
            counter_address,
            rpc_url: anvil.endpoint_url(),
            private_key: ctx.customer_signer,
            storage_config: StorageProviderConfig::builder()
                .storage_provider(StorageProviderType::Mock)
                .build()
                .unwrap(),
            deployment: Some(ctx.deployment),
        });

        // Run with properly handled cancellation.
        tokio::select! {
            run_result = run_task => run_result?,

            broker_task_result = tasks.join_next() => {
                panic!("Broker exited unexpectedly: {:?}", broker_task_result.unwrap());
            },

            _ = tokio::time::sleep(Duration::from_secs(TIMEOUT_SECS)) => {
                panic!("The run function did not complete within {} seconds", TIMEOUT_SECS)
            }
        }

        Ok(())
    }
}
