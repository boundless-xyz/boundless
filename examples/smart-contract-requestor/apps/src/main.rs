// Copyright (c) 2025 RISC Zero, Inc.
//
// All rights reserved.

use std::time::Duration;

use alloy::{
    network::Ethereum,
    primitives::{utils::parse_ether, Address, Bytes},
    providers::Provider,
    signers::local::PrivateKeySigner,
    sol_types::SolValue,
};
use anyhow::{bail, Context, Result};
use boundless_market::{
    client::{Client, ClientBuilder},
    contracts::{Input, Offer, Predicate, ProofRequest, RequestId, Requirements},
    storage::{StorageProvider, StorageProviderConfig},
};
use clap::Parser;
use guest_util::{ECHO_ELF, ECHO_ID};
use risc0_zkvm::{default_executor, serde::from_slice, sha::Digestible, Journal};
use url::Url;

/// Timeout for the transaction to be confirmed.
pub const TX_TIMEOUT: Duration = Duration::from_secs(30);

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
    /// Address of the smart contract requestor.
    #[clap(short, long, env)]
    smart_contract_requestor_address: Address,
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

    // NOTE: Using a separate `run` function to facilitate testing below.
    run(
        args.private_key,
        args.rpc_url,
        args.order_stream_url,
        args.storage_config,
        args.boundless_market_address,
        args.set_verifier_address,
        args.smart_contract_requestor_address,
    )
    .await?;

    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn run(
    private_key: PrivateKeySigner,
    rpc_url: Url,
    order_stream_url: Option<Url>,
    storage_config: Option<StorageProviderConfig>,
    boundless_market_address: Address,
    set_verifier_address: Address,
    smart_contract_requestor_address: Address,
) -> Result<()> {
    // Create a Boundless client from the provided parameters.
    let boundless_client = ClientBuilder::new()
        .with_storage_provider_config(storage_config.clone())
        .await?
        .with_rpc_url(rpc_url)
        .with_boundless_market_address(boundless_market_address)
        .with_set_verifier_address(set_verifier_address)
        .with_order_stream_url(order_stream_url)
        .with_private_key(private_key)
        .build()
        .await?;

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
    let request_id = RequestId::new(smart_contract_requestor_address, days_since_epoch)
        .set_smart_contract_signed_flag();

    // Create the requirements for the request. We use the predicate type `DigestMatch` to ensure that the journal
    // of the guest program matches a specific value. The pattern we use here is for our guest program to output the input
    // of the program as the journal. This allows us to validate that the correct input was used.
    //
    // In this example we expect the input of the program to be the current day since epoch, so we validate that
    // by creating a digest match predicate with days_since_epoch as the expected journal.
    //
    // When combined with the nonce structure of the request id, this ensures that
    // for each daily batch of work, the correct input was used.
    // Here we are using the echo guest, which simply echoes the input back.
    // Since for each day we want the input to the guest to be "days since epoch", and since the program just echoes
    // the input back, we can guarantee the correct input was used by checking that the output matches "days since epoch".
    let (mcycles_count, image_url, input_url, journal) =
        prepare_guest_input(&boundless_client, days_since_epoch).await?;
    let requirements = Requirements::new(ECHO_ID, Predicate::digest_match(journal.digest()));

    // Create the request, ensuring to set the request id and requirements that we prepared above.
    let request = ProofRequest::builder()
        .with_request_id(request_id)
        .with_image_url(image_url)
        .with_input(input_url)
        .with_requirements(requirements)
        .with_offer(
            Offer::default()
                .with_min_price_per_mcycle(parse_ether("0.001")?, mcycles_count)
                .with_max_price_per_mcycle(parse_ether("0.002")?, mcycles_count)
                .with_lock_timeout(1000)
                .with_timeout(2000)
                .with_bidding_start(now),
        )
        .build()?;

    // Send the request and wait for it to be completed.
    let signature: Bytes = request.abi_encode().into();
    let (request_id, expires_at) =
        boundless_client.submit_request_with_signature_bytes(&request, &signature).await?;
    tracing::info!("Request {} submitted", request_id);

    // Wait for the request to be fulfilled by the market. The market will return the journal and
    // seal.
    tracing::info!("Waiting for request {} to be fulfilled", request_id);
    let (_journal, seal) = boundless_client
        .wait_for_request_fulfillment(
            request_id,
            Duration::from_secs(5), // check every 5 seconds
            expires_at,
        )
        .await?;

    tracing::info!("Request {} fulfilled", request_id);
    tracing::info!("Seal: {:?}", seal);
    // We encoded the input to the guest as big endian for compatibility with Solidity. We reverse
    // to get back to little endian.
    let mut days_since_epoch_from_journal: u32 = from_slice(&_journal).unwrap();
    days_since_epoch_from_journal = days_since_epoch_from_journal.reverse_bits();
    tracing::info!("Journal Output: {:?} days since epoch", days_since_epoch_from_journal);

    Ok(())
}

async fn prepare_guest_input<P, S>(
    boundless_client: &Client<P, S>,
    days_since_epoch: u32,
) -> Result<(u64, Url, Url, Journal)>
where
    P: Provider<Ethereum> + 'static + Clone,
    S: StorageProvider + Clone,
{
    // Prepare the image and input for the guest program.
    let image_url =
        boundless_client.upload_image(ECHO_ELF).await.context("failed to upload image")?;

    // We encode the input as Big Endian, as this is how Solidity represents values. This simplifies validating
    // the requirements of the request in the smart contract client.
    let guest_env = Input::builder().write_slice(&days_since_epoch.to_be_bytes()).build_env()?;
    let input_url = boundless_client
        .upload_input(&guest_env.encode()?)
        .await
        .context("failed to upload input")?;

    // Execute the guest program to get the journal and session info.
    let session_info = default_executor().execute(guest_env.try_into()?, ECHO_ELF)?;
    let mcycles_count = session_info
        .segments
        .iter()
        .map(|segment| 1 << segment.po2)
        .sum::<u64>()
        .div_ceil(1_000_000);
    let journal = session_info.journal;

    Ok((mcycles_count, image_url, input_url, journal))
}

#[cfg(test)]
mod tests {
    use crate::tests::SmartContractRequestor::SmartContractRequestorInstance;
    use alloy::{
        network::EthereumWallet,
        node_bindings::{Anvil, AnvilInstance},
        primitives::Address,
        providers::{Provider, ProviderBuilder, WalletProvider},
        signers::local::PrivateKeySigner,
        sol_types::SolCall,
    };
    use boundless_market::contracts::{
        hit_points::default_allowance,
        test_utils::{create_test_ctx, TestCtx},
        IBoundlessMarket::{self},
    };
    use broker::test_utils::BrokerBuilder;
    use guest_assessor::ASSESSOR_GUEST_ID;
    use guest_set_builder::SET_BUILDER_ID;
    use tokio::time::timeout;
    use tracing_subscriber::EnvFilter;

    use super::*;

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
            .on_builtin(&anvil.endpoint())
            .await
            .unwrap();
        let smart_contract_requestor = SmartContractRequestor::deploy(
            &deployer_provider,
            deployer_address,
            test_ctx.boundless_market_address,
            0,
            100000,
        )
        .await?;

        Ok((*smart_contract_requestor.address(), deployer_provider.clone()))
    }

    #[tokio::test]
    // This test should run in dev mode, otherwise a storage provider and a prover backend are
    // required. To run in dev mode, set the `RISC0_DEV_MODE` environment variable to `true`,
    // e.g.: `RISC0_DEV_MODE=true cargo test`
    async fn test_main() {
        tracing_subscriber::fmt().with_env_filter(EnvFilter::from_default_env()).init();

        // Setup anvil and deploy contracts
        let anvil = Anvil::new().spawn();
        let ctx = create_test_ctx(&anvil, SET_BUILDER_ID, ASSESSOR_GUEST_ID).await.unwrap();
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
            .execute(ctx.boundless_market_address, deposit_call.into(), value_to_fund)
            .value(value_to_fund)
            .send()
            .await
            .unwrap();
        pending_deposit_tx.watch().await.unwrap();

        // Start a broker
        let (broker, _config_file) =
            BrokerBuilder::new_test(&ctx, anvil.endpoint_url()).await.build().await.unwrap();
        let broker_task = tokio::spawn(async move {
            broker.start_service().await.unwrap();
        });

        // Run the main function with a timeout of 60 seconds
        let result = timeout(
            Duration::from_secs(60),
            run(
                ctx.customer_signer,
                anvil.endpoint_url(),
                None,
                Some(StorageProviderConfig::dev_mode()),
                ctx.boundless_market_address,
                ctx.set_verifier_address,
                smart_contract_requestor_address,
            ),
        )
        .await;

        // Check the result of the timeout
        match result {
            Ok(run_result) => {
                // If the run completed, check for errors
                run_result.unwrap();
            }
            Err(_) => {
                // If timeout occurred, abort the broker task and fail the test
                broker_task.abort();
                panic!("The run function did not complete within 60 seconds.");
            }
        }

        // Check for a broker panic
        if broker_task.is_finished() {
            broker_task.await.unwrap();
        } else {
            broker_task.abort();
        }
    }
}
