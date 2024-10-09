// Copyright (c) 2024 RISC Zero, Inc.
//
// All rights reserved.

use std::time::{Duration, SystemTime};

use crate::counter::{ICounter, ICounter::ICounterInstance};
use alloy::{
    primitives::{aliases::U96, utils::parse_ether, Address, B256},
    signers::local::PrivateKeySigner,
    sol_types::SolCall,
};
use anyhow::{Context, Result};
use boundless_market::{
    contracts::{Input, Offer, Predicate, ProvingRequest, Requirements},
    sdk::client::Client,
};
use clap::Parser;
use guest_util::{ECHO_ELF, ECHO_ID};
use risc0_zkvm::{
    default_executor,
    sha::{Digest, Digestible},
    ExecutorEnv,
};
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
    #[clap(long, env)]
    requestor_private_key: PrivateKeySigner,
    /// Address of the Counter contract.
    #[clap(short, long, env)]
    counter_address: Address,
    /// Address of the SetVerifier contract.
    #[clap(short, long, env)]
    set_verifier_address: Address,
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

    // NOTE: Using a separate `run` function to facilitate testing below.
    run(
        args.requestor_private_key,
        args.rpc_url,
        args.proof_market_address,
        args.set_verifier_address,
        args.counter_address,
    )
    .await?;

    Ok(())
}

async fn run(
    requestor_private_key: PrivateKeySigner,
    rpc_url: Url,
    proof_market_address: Address,
    set_verifier_address: Address,
    counter_address: Address,
) -> Result<()> {
    // Create a Boundless client from the provided parameters.
    let boundless_client = Client::from_parts(
        requestor_private_key,
        rpc_url,
        proof_market_address,
        set_verifier_address,
    )
    .await?;

    // Upload the ECHO ELF to the storage provider so that it can be fetched by the market.
    let image_url = boundless_client.upload_image(ECHO_ELF).await?;
    tracing::info!("Uploaded image to {}", image_url);

    // We use a timestamp as input to the ECHO guest code as the Counter contract
    // accepts only unique proofs. Using the same input twice would result in the same proof.
    let timestamp = format! {"{:?}", SystemTime::now()};

    // Encode the input and upload it to the storage provider.
    let input = encode_input(timestamp.as_bytes())?;
    let input_url = boundless_client.upload_input(&input).await?;
    tracing::info!("Uploaded input to {}", input_url);

    // Dry run the ECHO ELF with the input to get the journal and cycle count.
    // This can be useful to estimate the cost of the proving request.
    // It can also be useful to ensure the guest can be executed correctly and we do not send into
    // the market unprovable proving requests. If you have a different mechanism to get the expected
    // journal and set a price, you can skip this step.
    let env = ExecutorEnv::builder().write_slice(&input).build()?;
    let session_info = default_executor().execute(env, ECHO_ELF)?;
    let mcycles_count = session_info
        .segments
        .iter()
        .map(|segment| 1 << segment.po2)
        .sum::<u64>()
        .div_ceil(1_000_000);
    let journal = session_info.journal;

    // Create a proving request with the image, input, requirements and offer.
    // The ELF (i.e. image) is specified by the image URL.
    // The input can be specified by an URL, as in this example, or can be posted on chain by using
    // the `with_inline` method with the input bytes.
    // The requirements are the ECHO_ID and the digest of the journal. In this way, the market can
    // verify that the proof is correct by checking both the committed image id and digest of the
    // journal. The offer specifies the price range and the timeout for the request.
    // Additionally, the offer can also specify:
    // - the bidding start time: the block number when the bidding starts;
    // - the ramp up period: the number of blocks before the price start increasing until reaches
    //   the maxPrice, starting from the the bidding start;
    // - the lockin price: the price at which the request can be locked in by a prover, if the
    //   request is not fulfilled before the timeout, the prover can be slashed.
    let request = ProvingRequest::default()
        .with_image_url(&image_url)
        .with_input(Input::url(&input_url))
        .with_requirements(Requirements::new(ECHO_ID, Predicate::digest_match(journal.digest())))
        .with_offer(
            Offer::default()
                // The market uses a reverse Dutch auction mechanism to match requests with provers.
                // Each request has a price range that a prover can bid on. One way to set the price
                // is to choose a desired (min and max) price per million cycles and multiply it
                // by the number of cycles. Alternatively, you can use the `with_min_price` and
                // `with_max_price` methods to set the price directly.
                .with_min_price_per_mcycle(
                    U96::from::<u128>(parse_ether("0.001")?.try_into()?),
                    mcycles_count,
                )
                // NOTE: If your offer is not being accepted, try increasing the max price.
                .with_max_price_per_mcycle(
                    U96::from::<u128>(parse_ether("0.002")?.try_into()?),
                    mcycles_count,
                )
                // The timeout is the maximum number of blocks the request can stay
                // unfulfilled in the market before it expires. If a prover locks in
                // the request and does not fulfill it before the timeout, the prover can be
                // slashed.
                .with_timeout(1000),
        );

    // Send the request and wait for it to be completed.
    let request_id = boundless_client.submit_request(&request).await?;
    tracing::info!("Request {} submitted", request_id);

    // Wait for the request to be fulfilled by the market. The market will return the journal and
    // seal.
    tracing::info!("Waiting for request {} to be fulfilled", request_id);
    let (_journal, seal) = boundless_client
        .wait_for_request_fulfillment(
            request_id,
            Duration::from_secs(5), // check every 5 seconds
            None,                   // no timeout
        )
        .await?;
    tracing::info!("Request {} fulfilled", request_id);

    // We interact with the Counter contract by calling the increment function with the journal and
    // seal returned by the market.
    let counter = ICounterInstance::new(counter_address, boundless_client.provider().clone());
    let journal_digest = B256::try_from(journal.digest().as_bytes())?;
    let image_id = B256::try_from(Digest::from(ECHO_ID).as_bytes())?;
    let call_increment =
        counter.increment(seal, image_id, journal_digest).from(boundless_client.caller());

    // By calling the increment function, we verify the seal against the published roots
    // of the SetVerifier contract.
    tracing::info!("Calling Counter increment function");
    let pending_tx = call_increment.send().await.context("failed to broadcast tx")?;
    tracing::info!("Broadcasting tx {}", pending_tx.tx_hash());
    let tx_hash =
        pending_tx.with_timeout(Some(TX_TIMEOUT)).watch().await.context("failed to confirm tx")?;
    tracing::info!("Tx {:?} confirmed", tx_hash);

    // Query the counter value for the caller address to check that the counter has been
    // increased.
    let count = counter
        .getCount(boundless_client.caller())
        .call()
        .await
        .with_context(|| format!("failed to call {}", ICounter::getCountCall::SIGNATURE))?
        ._0;
    tracing::info!("Counter value for address: {:?} is {:?}", boundless_client.caller(), count);

    Ok(())
}

// Encode the input as expected by the echo guest.
fn encode_input(input: &[u8]) -> Result<Vec<u8>> {
    Ok(bytemuck::pod_collect_to_vec(&risc0_zkvm::serde::to_vec(input)?))
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
    use broker::test_utils::broker_from_test_ctx;
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
            run(
                ctx.customer_signer,
                anvil.endpoint_url(),
                ctx.proof_market_addr,
                ctx.set_verifier_addr,
                counter_address,
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
