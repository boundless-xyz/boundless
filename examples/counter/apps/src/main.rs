// Copyright (c) 2024 RISC Zero, Inc.
//
// All rights reserved.

use std::time::{Duration, SystemTime};

use crate::counter::{ICounter, ICounter::ICounterInstance};
use alloy::{
    network::EthereumWallet,
    primitives::{utils::parse_units, Address, B256},
    providers::{Provider, ProviderBuilder},
    signers::local::PrivateKeySigner,
    sol_types::SolCall,
};
use anyhow::{Context, Result};
use boundless_market::{
    contracts::{
        proof_market::ProofMarketService, Input, InputType, Offer, Predicate, PredicateType,
        ProvingRequest, Requirements,
    },
    storage::{storage_provider_from_env, StorageProvider},
};
use clap::Parser;
use guest_util::{ECHO_ELF, ECHO_ID};
use risc0_zkvm::{
    default_executor,
    serde::to_vec,
    sha::{Digest, Digestible},
    ExecutorEnv,
};
use sha2::{Digest as _, Sha256};
use url::Url;

/// Timeout for the transaction to be confirmed.
pub const TX_TIMEOUT: Duration = Duration::from_secs(30);

mod counter {
    alloy::sol!(
        #![sol(rpc, all_derives)]
        "../contracts/src/ICounter.sol"
    );
}

/// Arguments of the publisher CLI.
#[derive(Parser, Debug)]
#[clap(author, version, about, long_about = None)]
struct Args {
    /// URL of the Ethereum RPC endpoint.
    #[clap(short, long, env)]
    rpc_url: Url,
    /// Private key used to interact with the Counter contract.
    #[clap(short, long, env)]
    wallet_private_key: PrivateKeySigner,
    /// Address of the Counter contract.
    #[clap(short, long, env)]
    counter_address: Address,
    /// Address of the ProofMarket contract.
    #[clap(short, long, env)]
    proof_market_address: Address,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    dotenvy::dotenv()?;
    let args = Args::parse();

    run(args.wallet_private_key, args.rpc_url, args.proof_market_address, args.counter_address)
        .await?;

    Ok(())
}

async fn run(
    wallet_private_key: PrivateKeySigner,
    rpc_url: Url,
    proof_market_address: Address,
    counter_address: Address,
) -> Result<()> {
    // We use a timestamp as input to the ECHO guest code as the Counter contract
    // accepts only unique proofs. Using the same input twice would result in the same proof.
    let image_id = B256::try_from(Digest::from(ECHO_ID).as_bytes())?;
    let timestamp = format! {"{:?}", SystemTime::now()};
    let input = timestamp.as_bytes();

    // Dry run with executor to ensure the guest can be executed correctly and we do not send into
    // the market unprovable requests. We also get the expected journal value here.
    let env = ExecutorEnv::builder().write(&input)?.build()?;
    let executor = default_executor();
    let session_info = executor.execute(env, ECHO_ELF)?;
    let journal_digest = session_info.journal.digest();

    // Setup to interact with the Market contract
    let caller = wallet_private_key.address();
    let signer = wallet_private_key.clone();
    let wallet = EthereumWallet::from(wallet_private_key);
    let provider =
        ProviderBuilder::new().with_recommended_fillers().wallet(wallet).on_http(rpc_url);
    let market = ProofMarketService::new(proof_market_address, provider.clone(), caller);

    // We create a proving request with the requirements and offer to send to the market.
    // The request requires to specify some requirements, e.g., the image id of the guest code
    // to prove as well as a predicate to satisfy. Currently we support two predcate types:
    // - 0: Digest match: the journal digest must be exactly the given bytes.
    // - 1: Prefix match: the journal must start with the given bytes.
    // In this example we provide the expected journal digest.
    let requirements = Requirements {
        imageId: image_id,
        predicate: Predicate {
            predicateType: PredicateType::DigestMatch,
            data: journal_digest.as_bytes().to_vec().into(),
        },
    };

    // The offer specifies the amount of tokens to pay for the proof [in wei], the expiration time
    // of the offer [in number of blocks], the lockin timeout [in number of blocks] and the
    // lockin stake [in wei]. The lockin stake is the amount of tokens to lockin by the broker that
    // wishes to acquire the exclusivity rights for the offer.
    let price = parse_units("0.001", "ether").unwrap();
    let current_block = provider.get_block_number().await?;
    let timeout = 1625190000;
    let offer = Offer {
        minPrice: price.try_into()?,
        maxPrice: price.try_into()?,
        biddingStart: current_block,
        rampUpPeriod: 0,
        timeout,
        lockinStake: parse_units("0", "ether").unwrap().try_into()?,
    };

    // Upload the guest code to the default storage provider.
    // It uses a temporary file storage provider if `RISC0_DEV_MODE` is set;
    // or if you'd like to use Pinata or S3 instead, you can set the appropriate env variables.
    let storage_provider = storage_provider_from_env().await?;
    let elf_url = storage_provider.upload_image(ECHO_ELF).await?;

    // Construct the request from its individual parts.
    let request = ProvingRequest::new(
        market.gen_random_id().await?,
        &caller,
        requirements,
        &elf_url,
        Input {
            inputType: InputType::Inline,
            data: bytemuck::pod_collect_to_vec(&to_vec(&input)?).into(),
        },
        offer,
    );

    // Send the request and wait for it to be completed.
    tracing::info!("Submitting request {} to the proof market", request.id);
    let request_id = market.submit_request(&request, &signer).await?;
    tracing::info!("Request {} submitted", request_id);

    // We wait for the request to be fulfilled by the market. The market will return the journal and
    // seal.
    tracing::info!("Waiting for request {} to be fulfilled", request_id);
    let (journal, seal) = market
        .wait_for_request_fulfillment(
            request_id,
            Duration::from_secs(5), // check every 5 seconds
            None,                   // no timeout
        )
        .await?;
    tracing::info!("Request {} fulfilled", request_id);

    // We interact with the Counter contract by calling the increment function with the journal and
    // seal returned by the market.
    let counter = ICounterInstance::new(counter_address, provider.clone());
    let journal_digest = B256::try_from(Sha256::digest(&journal).as_slice())?;
    let call_increment = counter.increment(seal, image_id, journal_digest).from(caller);

    // By calling the increment function, we verify the seal against the published roots
    // of the SetVerifier contract.
    tracing::info!("Calling Counter increment function");
    let pending_tx = call_increment.send().await.context("failed to broadcast tx")?;
    tracing::info!("Broadcasting tx {}", pending_tx.tx_hash());
    let tx_hash =
        pending_tx.with_timeout(Some(TX_TIMEOUT)).watch().await.context("failed to confirm tx")?;
    tracing::info!("Tx {:?} confirmed", tx_hash);

    // We query the counter value for the caller address to check that the counter has been
    // increased.
    let count = counter
        .getCount(caller)
        .call()
        .await
        .with_context(|| format!("failed to call {}", ICounter::getCountCall::SIGNATURE))?
        ._0;
    tracing::info!("Counter value for address: {:?} is {:?}", caller, count);

    Ok(())
}

#[cfg(test)]
mod tests {
    use alloy::{
        network::EthereumWallet,
        node_bindings::{Anvil, AnvilInstance},
        primitives::Address,
        providers::ProviderBuilder,
        signers::local::PrivateKeySigner,
    };
    use boundless_market::contracts::test_utils::TestCtx;
    use broker::broker_from_test_ctx;
    use tokio::time::timeout;
    use tracing_test::traced_test;

    use super::*;

    alloy::sol!(
        #![sol(rpc)]
        Counter,
        "../contracts/out/Counter.sol/Counter.json"
    );

    async fn deploy_counter(anvil: &AnvilInstance, test_ctx: &TestCtx) -> Result<Address> {
        let deployer_signer: PrivateKeySigner = anvil.keys()[0].clone().into();
        let deployer_provider = ProviderBuilder::new()
            .with_recommended_fillers()
            .wallet(EthereumWallet::from(deployer_signer))
            .on_builtin(&anvil.endpoint())
            .await
            .unwrap();
        let counter = Counter::deploy(&deployer_provider, test_ctx.set_verifier_addr).await?;

        Ok(*counter.address())
    }

    #[tokio::test]
    #[traced_test]
    // This test should run in dev mode, otherwise a storage provider and a prover backend are
    // required. To run in dev mode, set the `RISC0_DEV_MODE` environment variable to `true`,
    // e.g.: `RISC0_DEV_MODE=true cargo test`
    async fn test_main() {
        // Setup anvil and deploy contracts
        let anvil = Anvil::new().spawn();
        let ctx = TestCtx::new(&anvil).await.unwrap();
        let counter_address = deploy_counter(&anvil, &ctx).await.unwrap();

        // Start a broker
        let broker = broker_from_test_ctx(&ctx, anvil.endpoint_url()).await.unwrap();
        let broker_task = tokio::spawn(async move {
            broker.start_service().await.unwrap();
        });

        // Run the main function with a timeout of 60 seconds
        let result = timeout(
            Duration::from_secs(60),
            run(ctx.customer_signer, anvil.endpoint_url(), ctx.proof_market_addr, counter_address),
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
