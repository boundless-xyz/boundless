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

use std::{str::FromStr, time::Duration};

use alloy::{
    primitives::{Address, Bytes},
    signers::local::PrivateKeySigner,
    sol_types::SolValue,
};
use anyhow::{Context, Result};
use boundless_market::{Client, Deployment, RequestId, StorageProviderConfig};
use boundless_market_test_utils::ECHO_ELF;
use clap::Parser;
use risc0_zkvm::serde::from_slice;
use tracing_subscriber::{filter::LevelFilter, prelude::*, EnvFilter};
use url::Url;

/// Timeout for the transaction to be confirmed.
pub const TX_TIMEOUT: Duration = Duration::from_secs(30);

/// Arguments for the smart contract requestor CLI.
#[derive(Parser, Debug)]
#[clap(author, version, about, long_about = None)]
struct Args {
    /// URL of the Ethereum RPC endpoint.
    #[clap(short, long, env)]
    rpc_url: Url,
    /// Private key used to interact with the contracts and the Boundless Market.
    #[clap(long, env)]
    private_key: PrivateKeySigner,
    /// Address of the smart contract requestor.
    #[clap(short, long, env)]
    smart_contract_requestor_address: Address,
    /// Configuration for the StorageProvider to use for uploading programs and inputs.
    #[clap(flatten, next_help_heading = "Storage Provider")]
    storage_config: StorageProviderConfig,
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

    // NOTE: Using a separate `run` function to facilitate testing below.
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

    // For smart contract requestors, the request id is especially important as it acts as a nonce, ensuring that clients
    // do not pay for multiple requests that represent the same batch of work.
    //
    // This means you must be careful to design a nonce structure that maps to each batch of work you are requesting.
    //
    // For this simple example, we want one proof per day, so we use the index of the request id to represent each day
    // since the unix epoch, ensuring that we will only ever pay for one request per day.
    let now = std::time::SystemTime::now()
        .duration_since(std::time::SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let days_since_epoch = (now / (24 * 60 * 60)) as u32;

    // Create the request id, using days_since_epoch as the index, and with the smart contract signed flag set.
    // The smart contract signed flag is used to indicate that the request is "signed" by the smart contract
    // and must be validated using ERC-1271's isValidSignature function, and not a regular ECDSA recovery.
    let request_id = RequestId::new(args.smart_contract_requestor_address, days_since_epoch)
        .set_smart_contract_signed_flag();

    // Since for each day we want the input to the guest to be "days since epoch", and since the ECHO program just echoes
    // the input back, we can guarantee the correct input was used by checking that the output matches "days since epoch".
    // We use big-endian encoding for the input for compatibility with Solidity.
    let days_since_epoch_be = days_since_epoch.to_be_bytes();

    let request = client
        .new_request()
        .with_request_id(request_id)
        .with_program(ECHO_ELF)
        .with_stdin(days_since_epoch_be);

    // Send the request and wait for it to be completed.
    let request = client.build_request(request).await?;
    let signature: Bytes = request.abi_encode().into();
    let (request_id, expires_at) =
        client.submit_request_onchain_with_signature(&request, signature).await?;
    tracing::info!("Request {:x} submitted", request_id);

    // Wait for the request to be fulfilled by the market. The market will return the journal and seal.
    tracing::info!("Waiting for request {:x} to be fulfilled", request_id);
    let (journal, seal) = client
        .wait_for_request_fulfillment(
            request_id,
            Duration::from_secs(5), // check every 5 seconds
            expires_at,
        )
        .await?;

    tracing::info!("Request {:x} fulfilled", request_id);
    tracing::info!("Seal: {:?}", seal);

    // We encoded the input to the guest as big endian for compatibility with Solidity. We reverse
    // to get back to little endian.
    let mut days_since_epoch_from_journal: u32 = from_slice(&journal).unwrap();
    days_since_epoch_from_journal = days_since_epoch_from_journal.reverse_bits();
    tracing::info!("Journal Output: {:?} days since epoch", days_since_epoch_from_journal);

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::SmartContractRequestor::SmartContractRequestorInstance;
    use alloy::{
        network::EthereumWallet,
        node_bindings::{Anvil, AnvilInstance},
        primitives::{utils::parse_ether, Address},
        providers::{Provider, ProviderBuilder, WalletProvider},
        signers::local::PrivateKeySigner,
        sol_types::SolCall,
    };
    use boundless_market::contracts::{
        hit_points::default_allowance,
        IBoundlessMarket::{self},
    };
    use boundless_market::storage::StorageProviderType;
    use boundless_market_test_utils::{create_test_ctx, TestCtx};
    use broker::test_utils::BrokerBuilder;
    use test_log::test;
    use tokio::task::JoinSet;

    alloy::sol!(
        #![sol(rpc, all_derives)]
        SmartContractRequestor,
        "../contracts/out/SmartContractRequestor.sol/SmartContractRequestor.json"
    );

    // Returns the address of the smart contract requestor and the provider of the owner of the contract.
    async fn deploy_smart_contract_requestor<P: Provider + 'static + Clone + WalletProvider>(
        anvil: &AnvilInstance,
        test_ctx: &TestCtx<P>,
    ) -> Result<(Address, impl Provider + WalletProvider + Clone + 'static)> {
        let deployer_signer: PrivateKeySigner = anvil.keys()[0].clone().into();
        let deployer_address = deployer_signer.address();
        let deployer_provider = ProviderBuilder::new()
            .wallet(EthereumWallet::from(deployer_signer))
            .connect(&anvil.endpoint())
            .await
            .unwrap();
        let smart_contract_requestor = SmartContractRequestor::deploy(
            &deployer_provider,
            deployer_address,
            test_ctx.deployment.boundless_market_address,
            0,
            100000,
        )
        .await?;

        Ok((*smart_contract_requestor.address(), deployer_provider.clone()))
    }

    #[test(tokio::test)]
    async fn test_main() -> Result<()> {
        // Setup anvil and deploy contracts
        let anvil = Anvil::new().spawn();
        let ctx = create_test_ctx(&anvil).await.unwrap();
        ctx.prover_market
            .deposit_stake_with_permit(default_allowance(), &ctx.prover_signer)
            .await
            .unwrap();
        let (smart_contract_requestor_address, smart_contract_requestor_owner) =
            deploy_smart_contract_requestor(&anvil, &ctx).await.unwrap();

        let value_to_fund = parse_ether("0.5").unwrap();

        let smart_contract_requestor = SmartContractRequestorInstance::new(
            smart_contract_requestor_address,
            smart_contract_requestor_owner.clone(),
        )
        .clone();

        // Fund the smart contract client with ETH and deposit to the market.
        let deposit_call = IBoundlessMarket::depositCall {}.abi_encode();

        let pending_deposit_tx = smart_contract_requestor
            .execute(ctx.deployment.boundless_market_address, deposit_call.into(), value_to_fund)
            .value(value_to_fund)
            .send()
            .await
            .unwrap();
        pending_deposit_tx.watch().await.unwrap();

        // A JoinSet automatically aborts all its tasks when dropped
        let mut tasks = JoinSet::new();

        // Start a broker
        let (broker, _config) =
            BrokerBuilder::new_test(&ctx, anvil.endpoint_url()).await.build().await?;
        tasks.spawn(async move { broker.start_service().await });

        const TIMEOUT_SECS: u64 = 300; // 5 minutes

        // Create test args for the run function
        let run_args = Args {
            rpc_url: anvil.endpoint_url(),
            private_key: ctx.customer_signer,
            smart_contract_requestor_address,
            storage_config: StorageProviderConfig::builder()
                .storage_provider(StorageProviderType::Mock)
                .build()
                .unwrap(),
            deployment: Some(ctx.deployment),
        };

        // Run with properly handled cancellation.
        tokio::select! {
            run_result = run(run_args) => run_result?,

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
