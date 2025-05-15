// Copyright (c) 2025 RISC Zero, Inc.
//
// All rights reserved.

use std::time::{Duration, SystemTime};

use crate::ICounter::ICounterInstance;
use alloy::{
    network::Ethereum,
    primitives::{utils::parse_ether, Address, Bytes, B256},
    providers::Provider,
    signers::local::PrivateKeySigner,
    sol_types::SolCall,
};
use anyhow::{bail, Context, Result};
use boundless_market::storage::BuiltinStorageProvider;
use boundless_market::{
    client::Client,
    contracts::{Input, Offer, Predicate, ProofRequest, Requirements},
    input::GuestEnv,
    storage::{StorageProvider, StorageProviderConfig},
};
use clap::Parser;
use guest_util::{ECHO_ELF, ECHO_ID, IDENTITY_ELF, IDENTITY_ID};
use risc0_ethereum_contracts::receipt::Receipt as ContractReceipt;
use risc0_zkvm::{
    compute_image_id, default_executor,
    sha::{Digest, Digestible},
    ExecutorEnv, Journal,
};
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
    /// URL of the offchain order stream endpoint.
    #[clap(short, long, env)]
    order_stream_url: Option<Url>,
    /// Storage provider configuration
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

    load_dotenv()?;
    let args = Args::parse();

    // NOTE: Using a separate `run` function to facilitate testing.
    run(
        args.private_key,
        args.rpc_url,
        args.order_stream_url,
        BuiltinStorageProvider::from_config(&args.storage_config)?,
        args.boundless_market_address,
        args.set_verifier_address,
        args.counter_address,
    )
    .await?;

    Ok(())
}

/// Load environment variables from a `.env` file if available.
fn load_dotenv() -> Result<()> {
    match dotenvy::dotenv() {
        Ok(path) => tracing::debug!("Loaded environment variables from {:?}", path),
        Err(e) if e.not_found() => {
            tracing::debug!("No .env file found");
        }
        Err(e) => bail!("Failed to load .env file: {}", e),
    }
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
    let boundless_client = Client::builder()
        .with_rpc_url(rpc_url)
        .with_boundless_market_address(boundless_market_address)
        .with_set_verifier_address(set_verifier_address)
        .with_order_stream_url(order_stream_url)
        .with_storage_provider(Some(storage_provider))
        .with_private_key(private_key)
        .build()
        .await
        .context("failed to build boundless client")?;

    // We use a timestamp as input to the ECHO guest code so that the proof is unique.
    let echo_input = Vec::from(format!("{:?}", SystemTime::now()));
    let echo_guest_env = Input::builder().write_slice(&echo_input).build_env()?;

    // Request an un-aggregated proof from the Boundless market using the ECHO guest.
    let (echo_journal, echo_seal) =
        boundless_proof(&boundless_client, ECHO_ELF, echo_guest_env, true)
            .await
            .context("failed to prove ECHO")?;

    // Decode the resulting RISC0-ZKVM receipt.
    let Ok(ContractReceipt::Base(echo_receipt)) = risc0_ethereum_contracts::receipt::decode_seal(
        echo_seal,
        ECHO_ID,
        echo_journal.bytes.clone(),
    ) else {
        bail!("did not receive requested unaggregated receipt");
    };
    let echo_claim_digest = echo_receipt.claim().unwrap().digest();

    // Build the IDENTITY input with from the ECHO receipt.
    let identity_input = (Digest::from(ECHO_ID), echo_receipt);
    let identity_guest_env =
        Input::builder().write_frame(&postcard::to_allocvec(&identity_input)?).build_env()?;

    // Request a proof from the Boundless market using the IDENTITY guest.
    let (identity_journal, identity_seal) =
        boundless_proof(&boundless_client, IDENTITY_ELF, identity_guest_env, false)
            .await
            .context("failed to prove IDENTITY")?;
    debug_assert_eq!(&identity_journal.bytes, echo_claim_digest.as_bytes());

    // Interact with the Counter contract by calling the increment function.
    let counter = ICounterInstance::new(counter_address, boundless_client.provider());
    let journal_digest = B256::from_slice(identity_journal.digest().as_bytes());
    let image_id = B256::from_slice(Digest::from(IDENTITY_ID).as_bytes());
    let call_increment =
        counter.increment(identity_seal, image_id, journal_digest).from(boundless_client.caller());

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
        .getCount(boundless_client.caller())
        .call()
        .await
        .with_context(|| format!("failed to call {}", ICounter::getCountCall::SIGNATURE))?;
    tracing::info!("Counter value for address {:?} is {:?}", boundless_client.caller(), count);

    Ok(())
}

/// Execute the Boundless market prove process.
/// This function uploads the program and input, runs the guest executor, builds the request,
/// submits it, and waits for the fulfillment.
async fn boundless_proof<P, S>(
    client: &Client<P, S>,
    program: impl AsRef<[u8]>,
    guest_env: GuestEnv,
    groth16: bool,
) -> Result<(Journal, Bytes)>
where
    P: Provider<Ethereum> + 'static + Clone,
    S: StorageProvider,
{
    // Compute the image ID of the program
    let program = program.as_ref();
    let image_id =
        compute_image_id(program).context("failed to compute image ID from provided ELF")?;

    // Upload the ELF binary and input data
    let program_url = client.upload_program(program).await.context("failed to upload program")?;
    tracing::info!("Uploaded program to {}", program_url);

    let input_encoded = guest_env.encode().context("failed to encode input")?;
    let input_url = client.upload_input(&input_encoded).await.context("failed to upload input")?;
    tracing::info!("Uploaded input to {}", input_url);

    // Execute the guest binary with the input
    let mut env_builder = ExecutorEnv::builder();
    env_builder.write_slice(&guest_env.stdin);

    let session_info = default_executor()
        .execute(env_builder.build()?, program)
        .context("failed to execute ELF")?;
    // Calculate the cycles (in millions) required.
    let mcycles_count = session_info
        .segments
        .iter()
        .map(|segment| 1 << segment.po2)
        .sum::<u64>()
        .div_ceil(1_000_000);
    let journal = session_info.journal;

    // Build the proof requirements with the specified selector
    let mut requirements = Requirements::new(image_id, Predicate::digest_match(journal.digest()));
    if groth16 {
        requirements = requirements.with_groth16_proof();
    }

    // Build the proof request offer
    let offer = Offer::default()
        // The market uses a reverse Dutch auction mechanism. Set min and max prices per million cycles.
        .with_min_price_per_mcycle(parse_ether("0.001")?, mcycles_count)
        // NOTE: If your offer is not being accepted, try increasing the max price.
        .with_max_price_per_mcycle(parse_ether("0.002")?, mcycles_count)
        // Timeouts for the request and lock.
        .with_timeout(1200)
        .with_lock_timeout(1200);

    // Build and submit the request
    let request = ProofRequest::builder()
        .with_image_url(program_url)
        .with_input(input_url)
        .with_requirements(requirements)
        .with_offer(offer)
        .build()?;
    let (request_id, expires_at) = client.submit_request(&request).await?;
    tracing::info!("Request {} submitted", request_id);

    // Wait for the request to be fulfilled (check periodically)
    tracing::info!("Waiting for request {} to be fulfilled", request_id);
    let (_, seal) = client
        .wait_for_request_fulfillment(
            request_id,
            Duration::from_secs(5), // periodic check every 5 seconds
            expires_at,
        )
        .await?;
    tracing::info!("Request {} fulfilled", request_id);

    Ok((journal, seal))
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::{
        network::EthereumWallet,
        node_bindings::{Anvil, AnvilInstance},
        providers::{ProviderBuilder, WalletProvider},
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

        const TIMEOUT_SECS: u64 = 1800; // 30 minutes

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
