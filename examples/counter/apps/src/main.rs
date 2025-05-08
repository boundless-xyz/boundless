// Copyright (c) 2025 RISC Zero, Inc.
//
// All rights reserved.

use std::time::{Duration, SystemTime};

use crate::counter::{ICounter, ICounter::ICounterInstance};
use alloy::{
    primitives::{utils::parse_ether, Address, B256},
    signers::local::PrivateKeySigner,
    sol_types::SolCall,
};
use anyhow::{bail, Context, Result};
use boundless_market::{
    client::ClientBuilder,
    contracts::{Input, Offer, Predicate, ProofRequest, Requirements},
    storage::{BuiltinStorageProvider, StorageProvider, StorageProviderConfig},
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
    /// URL of the offchain order stream endpoint.
    #[clap(short, long, env)]
    order_stream_url: Option<Url>,
    /// Storage provider to use
    #[clap(flatten)]
    storage_config: StorageProviderConfig,
    /// Private key used to interact with the Counter contract.
    #[clap(long, env)]
    private_key: PrivateKeySigner,
    /// Address of the Counter contract.
    #[clap(short, long, env)]
    counter_address: Address,
    /// Address of the SetVerifier contract.
    #[clap(short, long, env)]
    set_verifier_address: Address,
    /// Address of the BoundlessMarket contract.
    #[clap(short, long, env)]
    boundless_market_address: Address,
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
        BuiltinStorageProvider::from_config(&args.storage_config).await?,
        args.boundless_market_address,
        args.set_verifier_address,
        args.counter_address,
    )
    .await?;

    Ok(())
}

/// Main logic which creates the Boundless client, executes the proofs and submits the tx.
async fn run<P: StorageProvider>(
    private_key: PrivateKeySigner,
    rpc_url: Url,
    order_stream_url: Option<Url>,
    storage_provider: P,
    boundless_market_address: Address,
    set_verifier_address: Address,
    counter_address: Address,
) -> Result<()> {
    // Create a Boundless client from the provided parameters.
    let client = ClientBuilder::<P>::default()
        .with_rpc_url(rpc_url)
        .with_boundless_market_address(boundless_market_address)
        .with_set_verifier_address(set_verifier_address)
        .with_order_stream_url(order_stream_url)
        .with_storage_provider(Some(storage_provider))
        .with_private_key(private_key)
        .build()
        .await
        .context("failed to build boundless client")?;

    // Upload the ECHO program to the storage provider so that it can be fetched by the market.
    let program_url = client.upload_program(ECHO_ELF).await.context("failed to upload program")?;
    tracing::info!("Uploaded program to {}", program_url);

    // We use a timestamp as input to the ECHO guest code as the Counter contract
    // accepts only unique proofs. Using the same input twice would result in the same proof.
    let echo_message = format! {"{:?}", SystemTime::now()};
    let echo_input = Input::builder().write_slice(echo_message.as_bytes()).build_env()?;

    // Encode the input and upload it to the storage provider.
    let input_encoded = echo_input.encode().context("failed to encode input")?;
    let input_url = client.upload_input(&input_encoded).await.context("failed to upload input")?;
    tracing::info!("Uploaded input to {}", input_url);

    // Dry run the ECHO program with the input to get the journal and cycle count.
    // This can be useful to estimate the cost of the poof request.
    // It can also be useful to ensure the guest can be executed correctly and we do not send into
    // the market unprovable proof requests. If you have a different mechanism to get the expected
    // journal and set a price, you can skip this step.
    let env = ExecutorEnv::builder().write_slice(&echo_input.stdin).build()?;
    let session_info =
        default_executor().execute(env, ECHO_ELF).context("failed to execute program")?;
    let mcycles_count = session_info
        .segments
        .iter()
        .map(|segment| 1 << segment.po2)
        .sum::<u64>()
        .div_ceil(1_000_000);
    let journal = session_info.journal;

    // Create a proof request with the image, input, requirements and offer.
    // The program (i.e. image) is specified by the image URL.
    // The input can be specified by an URL, as in this example, or can be posted on chain by using
    // the `with_inline` method with the input bytes.
    // The requirements are the ECHO_ID and the digest of the journal. In this way, the market can
    // verify that the proof is correct by checking both the committed image id and digest of the
    // journal. The offer specifies the price range and the timeout for the request.
    // Additionally, the offer can also specify:
    // - the bidding start time: the UNIX timestamp at which the bidding starts;
    // - the ramp up period: the number of seconds before the price increases until reaches
    //   the maxPrice, starting from the bidding start;
    // - the lockin price: the price at which the request can be locked in by a prover, if the
    //   request is not fulfilled before the timeout, the prover can be slashed.
    let request = ProofRequest::builder()
        .with_image_url(program_url)
        .with_input(input_url)
        .with_requirements(Requirements::new(ECHO_ID, Predicate::digest_match(journal.digest())))
        .with_offer(
            Offer::default()
                // The market uses a reverse Dutch auction mechanism to match requests with provers.
                // Each request has a price range that a prover can bid on. One way to set the price
                // is to choose a desired (min and max) price per million cycles and multiply it
                // by the number of cycles. Alternatively, you can use the `with_min_price` and
                // `with_max_price` methods to set the price directly.
                .with_min_price_per_mcycle(parse_ether("0.001")?, mcycles_count)
                // NOTE: If your offer is not being accepted, try increasing the max price.
                .with_max_price_per_mcycle(parse_ether("0.002")?, mcycles_count)
                // The timeout is the maximum number of seconds the request can stay
                // unfulfilled in the market before it expires. If a prover locks in
                // the request and does not fulfill it before the timeout, the prover can be
                // slashed.
                .with_timeout(1000)
                .with_lock_timeout(1000),
        )
        .build()?;

    // Send the request and wait for it to be completed.
    let (request_id, expires_at) = client.submit_request(&request).await?;
    tracing::info!("Request {} submitted", request_id);

    // Wait for the request to be fulfilled by the market. The market will return the journal and
    // seal.
    tracing::info!("Waiting for request {} to be fulfilled", request_id);
    let (_journal, seal) = client
        .wait_for_request_fulfillment(
            request_id,
            Duration::from_secs(5), // check every 5 seconds
            expires_at,
        )
        .await?;
    tracing::info!("Request {} fulfilled", request_id);

    // We interact with the Counter contract by calling the increment function with the journal and
    // seal returned by the market.
    let counter = ICounterInstance::new(counter_address, client.provider().clone());
    let journal_digest = B256::try_from(journal.digest().as_bytes())?;
    let image_id = B256::try_from(Digest::from(ECHO_ID).as_bytes())?;
    let call_increment = counter.increment(seal, image_id, journal_digest).from(client.caller());

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
        .getCount(client.caller())
        .call()
        .await
        .with_context(|| format!("failed to call {}", ICounter::getCountCall::SIGNATURE))?
        ._0;
    tracing::info!("Counter value for address: {:?} is {:?}", client.caller(), count);

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
    use boundless_market::contracts::hit_points::default_allowance;
    use boundless_market::storage::MockStorageProvider;
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
        let counter = Counter::deploy(&deployer_provider, test_ctx.set_verifier_address).await?;

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

        const TIMEOUT_SECS: u64 = 600; // 10 minutes

        // Run with properly handled cancellation.
        tokio::select! {
            run_result = run(
                ctx.customer_signer,
                anvil.endpoint_url(),
                None,
                MockStorageProvider::start(),
                ctx.boundless_market_address,
                ctx.set_verifier_address,
                counter_address,
            ) => run_result?,

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
