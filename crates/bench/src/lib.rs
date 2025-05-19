// Copyright (c) 2025 RISC Zero, Inc.
//
// All rights reserved.

use std::{collections::HashSet, fs::File, path::PathBuf, time::Duration};

use alloy::{
    network::EthereumWallet,
    primitives::{
        utils::{format_units, parse_ether},
        Address, U256,
    },
    providers::Provider,
    signers::local::PrivateKeySigner,
    sol_types::SolStruct,
};
use anyhow::{anyhow, bail, Result};
use boundless_indexer::{IndexerService, IndexerServiceConfig};
use boundless_market::{
    balance_alerts_layer::BalanceAlertConfig,
    client::ClientBuilder,
    contracts::{Input, Offer, Predicate, ProofRequest, Requirements},
    input::InputBuilder,
    storage::{
        storage_provider_from_config, storage_provider_from_env, BuiltinStorageProvider,
        StorageProviderConfig,
    },
};
use clap::Parser;
use futures::future::try_join_all;
use risc0_zkvm::{compute_image_id, default_executor, serde::to_vec, sha::Digestible, Journal};
use tempfile::NamedTempFile;
use tokio::{signal, task::JoinHandle, time::Instant};
use url::Url;

mod bench;
pub mod db;

pub use bench::{Bench, BenchRow, BenchRows};

const LOCK_FULFILL_GAS_UPPER_BOUND: u128 = 100_000_000;

/// Arguments of the benchmark.
#[derive(Parser, Debug)]
#[clap(author, version, about, long_about = None)]
pub struct MainArgs {
    /// URL of the Ethereum RPC endpoint.
    #[clap(short, long, env)]
    pub rpc_url: Url,
    /// Optional URL of the offchain order stream endpoint.
    ///
    /// If set, the order-generator will submit requests off-chain.
    #[clap(short, long, env)]
    pub order_stream_url: Option<Url>,
    /// Private key used to sign and submit requests.
    ///
    /// The key must be in hex format, without the 0x prefix.
    #[clap(short, long, env)]
    pub private_key: PrivateKeySigner,
    /// Address of the SetVerifier contract.
    #[clap(short, long, env)]
    pub set_verifier_address: Address,
    /// Address of the BoundlessMarket contract.
    #[clap(short, long, env)]
    pub boundless_market_address: Address,
    // Storage provider to use.
    #[clap(flatten)]
    pub storage_config: Option<StorageProviderConfig>,
    /// The DB url of the indexer to use.
    ///
    /// If not set, a new indexer instance is spawn locally.
    #[clap(long)]
    pub indexer_url: Option<String>,
    /// The path for the sqlite file of the indexer to use.
    ///
    /// If set, the indexer will use this file for its database.
    /// If not set, a new temporary sqlite file is used.
    #[clap(long, env, conflicts_with = "indexer_url")]
    pub sqlite_path: Option<PathBuf>,
    /// Program binary file to use as the guest image, given as a path.
    ///
    /// If unspecified, defaults to the included loop guest.
    #[clap(long)]
    pub program: Option<PathBuf>,
    /// Balance threshold at which to log a warning.
    #[clap(long, value_parser = parse_ether, default_value = "1")]
    pub warn_balance_below: Option<U256>,
    /// Balance threshold at which to log an error.
    #[clap(long, value_parser = parse_ether, default_value = "0.1")]
    pub error_balance_below: Option<U256>,
    /// The path to the benchmark config file.
    #[clap(long)]
    pub bench: PathBuf,
    /// The path of the output file.
    #[clap(long)]
    pub output: Option<PathBuf>,
    /// Use json format for the output file.
    #[clap(long)]
    pub json: bool,
    /// Estimate the cost for running the benchmark.
    ///
    /// If set, the benchmark will not be run, but the cost will be estimated.
    /// This is useful for testing the cost of a benchmark without actually running it.
    #[clap(long)]
    pub estimate: bool,
}

pub async fn run(args: &MainArgs) -> Result<()> {
    let storage_provider = match &args.storage_config {
        Some(storage_config) => storage_provider_from_config(storage_config).await?,
        None => storage_provider_from_env().await?,
    };
    let private_key = args.private_key.clone();
    let wallet = EthereumWallet::from(private_key.clone());
    let balance_alerts = BalanceAlertConfig {
        watch_address: wallet.default_signer().address(),
        warn_threshold: args.warn_balance_below,
        error_threshold: args.error_balance_below,
    };
    let boundless_client = ClientBuilder::<BuiltinStorageProvider>::new()
        .with_rpc_url(args.rpc_url.clone())
        .with_storage_provider(Some(storage_provider.clone()))
        .with_boundless_market_address(args.boundless_market_address)
        .with_set_verifier_address(args.set_verifier_address)
        .with_order_stream_url(args.order_stream_url.clone())
        .with_private_key(private_key)
        .with_balance_alerts(balance_alerts)
        .build()
        .await?;

    let (program, program_url) = match &args.program {
        Some(path) => {
            let program = std::fs::read(path)?;
            let program_url = boundless_client.upload_program(&program).await?;
            tracing::debug!("Uploaded program to {}", program_url);
            (program, program_url)
        }
        None => {
            // A build of the loop guest, which simply loop until reaching the cycle count it reads from inputs and commits to it.
            let url = "https://gateway.pinata.cloud/ipfs/bafkreicmwk3xlxbozbp5h63xyywocc7dltt376hn4mnmhk7ojqdcbrkqzi";
            (fetch_http(&Url::parse(url)?).await?, Url::parse(url)?)
        }
    };
    let image_id = compute_image_id(&program)?;

    let bench_file = File::open(&args.bench)?;
    let bench: Bench = serde_json::from_reader(bench_file)?;
    let input = bench.cycle_count_per_request;
    let env = InputBuilder::new().write(&(input as u64))?.write(&now_timestamp())?.build_env()?;
    let session_info = default_executor().execute(env.clone().try_into()?, &program)?;

    let cycles_count = session_info.segments.iter().map(|segment| 1 << segment.po2).sum::<u64>();
    let min_price = parse_ether(&bench.min_price_per_mcycle)?
        .checked_mul(U256::from(cycles_count))
        .unwrap()
        .div_ceil(U256::from(1_000_000));
    let mcycle_max_price = parse_ether(&bench.max_price_per_mcycle)?
        .checked_mul(U256::from(cycles_count))
        .unwrap()
        .div_ceil(U256::from(1_000_000));

    // start the indexer
    let temp_db = NamedTempFile::new().unwrap();
    let domain = boundless_client.boundless_market.eip712_domain().await?;
    let db_file_path: PathBuf =
        args.sqlite_path.as_ref().map_or_else(|| temp_db.path().to_path_buf(), |p| p.clone());
    if !db_file_path.exists() {
        std::fs::create_dir_all(db_file_path.parent().unwrap())?;
    }
    let db_url = format!("sqlite:{}", db_file_path.display());
    let current_block = boundless_client.provider().get_block_number().await?;
    let (indexer_url, indexer_handle): (String, Option<JoinHandle<Result<()>>>) =
        match args.indexer_url.clone() {
            Some(url) => (url, None),
            None => {
                let mut indexer = IndexerService::new(
                    args.rpc_url.clone(),
                    &PrivateKeySigner::random(),
                    args.boundless_market_address,
                    &db_url,
                    IndexerServiceConfig { interval: Duration::from_secs(2), retries: 5 },
                )
                .await?;

                let handle: JoinHandle<Result<()>> = tokio::spawn(async move {
                    if let Err(err) = indexer.run(Some(current_block)).await {
                        bail!("Error running the indexer: {}", err);
                    }
                    Ok(())
                });

                (db_url, Some(handle))
            }
        };
    tracing::debug!("Indexer URL: {}", indexer_url);

    //force auto-deposit
    let gas_price: u128 = boundless_client.provider().get_gas_price().await?;
    let gas_cost_estimate = (gas_price + gas_price / 10) * LOCK_FULFILL_GAS_UPPER_BOUND;
    let max_price = mcycle_max_price + U256::from(gas_cost_estimate);
    let amount: alloy::primitives::Uint<256, 4> = max_price * U256::from(bench.requests_count);
    let current_balance =
        boundless_client.boundless_market.balance_of(boundless_client.caller()).await?;
    let deposit_value =
        if current_balance < amount { amount - current_balance } else { U256::ZERO };

    if args.estimate {
        tracing::info!(
            "Estimated cost for {} requests: {} ETH",
            bench.requests_count,
            format_units(amount, "ether")?
        );
        tracing::info!("Current balance: {} ETH", format_units(current_balance, "ether")?);
        tracing::info!("Deposit required: {} ETH", format_units(deposit_value, "ether")?);
        return Ok(());
    }

    if deposit_value > U256::ZERO {
        tracing::info!(
            "Caller {} balance {} ETH is below estimated target {} ETH, depositing {} ETH...",
            boundless_client.caller(),
            format_units(current_balance, "ether")?,
            format_units(amount, "ether")?,
            format_units(deposit_value, "ether")?
        );
        boundless_client.boundless_market.deposit(deposit_value).await.expect("Failed to deposit");
    }

    let balance_before =
        boundless_client.boundless_market.balance_of(boundless_client.caller()).await?;
    tracing::info!(
        "Caller {} balance before bench: {} ETH",
        boundless_client.caller(),
        format_units(balance_before, "ether")?
    );

    let timeout = bench.timeout;
    let lock_timeout = bench.lock_timeout;
    let interval = bench.interval;
    let lockin_stake = parse_ether(&bench.lockin_stake)?;
    let ramp_up = bench.ramp_up;
    let threads = bench.threads as usize;
    let boundless_market_address = args.boundless_market_address;

    // Spawn one task per thread
    // Each task will handle its own requests
    let mut tasks: Vec<JoinHandle<anyhow::Result<Vec<BenchRow>>>> = Vec::with_capacity(threads);
    for i in 0..threads {
        // Pre-clone everything each task will need
        let client = boundless_client.clone();
        let program_url = program_url.clone();
        let domain = domain.clone();
        let order_stream = args.order_stream_url.clone();

        tasks.push(tokio::spawn(async move {
            let mut rows: Vec<BenchRow> = Vec::new();

            for i in (i..bench.requests_count as usize).step_by(threads) {
                let bidding_start = now_timestamp() + 10;
                let env =
                    InputBuilder::new().write(&(i as u64))?.write(&bidding_start)?.build_env()?;
                let journal = Journal::new(bytemuck::pod_collect_to_vec(&to_vec(&(
                    i as u64,
                    bidding_start,
                ))?));

                let mut request = ProofRequest::builder()
                    .with_image_url(program_url.clone())
                    .with_input(Input::inline(env.encode()?))
                    .with_requirements(Requirements::new(
                        image_id,
                        Predicate::digest_match(journal.digest()),
                    ))
                    .with_offer(
                        Offer::default()
                            .with_bidding_start(bidding_start)
                            .with_min_price(min_price)
                            .with_max_price(max_price)
                            .with_lock_stake(lockin_stake)
                            .with_ramp_up_period(ramp_up)
                            .with_timeout(timeout)
                            .with_lock_timeout(lock_timeout),
                    )
                    .build()?;
                tracing::debug!("Request: {:?}", request);

                let res = if order_stream.is_some() {
                    client.submit_request_offchain(&request).await
                } else {
                    client.submit_request(&request).await
                };
                if let Err(e) = res {
                    tracing::error!("Error submitting request: {}", e);
                    continue;
                }
                let (request_id, expires_at) = res.unwrap();
                request.id = request_id;

                let digest = request.eip712_signing_hash(&domain.alloy_struct());
                rows.push(BenchRow::new(
                    format!("{digest:x}"),
                    format!("{request_id:x}"),
                    i as u64,
                    request.offer.biddingStart,
                    expires_at,
                ));

                if order_stream.is_some() {
                    tracing::info!(
                        "Request 0x{request_id:x} submitted offchain to {} - {}/{}",
                        order_stream.clone().unwrap(),
                        i + 1,
                        bench.requests_count
                    );
                } else {
                    tracing::info!(
                        "Request 0x{request_id:x} submitted onchain to {} - {}/{}",
                        boundless_market_address,
                        i + 1,
                        bench.requests_count
                    );
                }

                tokio::time::sleep(Duration::from_secs(interval)).await;
            }

            Ok(rows)
        }));
    }

    // Wait for all tasks
    let bench_rows: Vec<BenchRow> = {
        let rows_results = try_join_all(tasks).await?;

        rows_results
            .into_iter()
            .map(|task_res| task_res.map_err(|e| anyhow!("Task failed: {}", e)))
            .collect::<Result<Vec<Vec<BenchRow>>, anyhow::Error>>()?
            .into_iter()
            .flatten()
            .collect()
    };

    tracing::info!("All submissions dispatched; waiting for fulfillment…");
    wait(&bench_rows, &indexer_url, bench.timeout).await?;

    let processed = process(&bench_rows, &indexer_url).await?;
    tracing::info!("Bench complete");
    let balance_after =
        boundless_client.boundless_market.balance_of(boundless_client.caller()).await?;
    tracing::info!(
        "Caller {} balance after bench: {} ETH (change: -{})",
        boundless_client.caller(),
        format_units(balance_after, "ether")?,
        format_units(balance_before - balance_after, "ether")?
    );
    tracing::info!("Writing results to file...");
    processed.dump(args.output.clone(), args.json)?;

    if let Some(handle) = indexer_handle {
        handle.abort();
    }

    Ok(())
}

async fn wait(rows: &[BenchRow], db_url: &str, timeout_secs: u32) -> Result<()> {
    let db = db::Monitor::new(db_url).await?;
    let mut pending: HashSet<String> = rows.iter().map(|r| r.request_digest.clone()).collect();
    let deadline = Instant::now() + Duration::from_secs((timeout_secs).into());

    while !pending.is_empty() && Instant::now() < deadline {
        let mut just_fulfilled = Vec::new();
        for digest in &pending {
            if db.fetch_fulfilled_at(digest).await?.is_some() {
                just_fulfilled.push(digest.clone());
            }
        }

        for d in just_fulfilled {
            pending.remove(&d);
        }

        if pending.is_empty() {
            break;
        }

        tracing::info!("Still waiting on {} requests...", pending.len());
        tokio::select! {
                    _ = signal::ctrl_c() => {
                         tracing::info!("Ctrl-C received, aborting wait early");
                         return Ok(());
                    }
                    _ = tokio::time::sleep(Duration::from_secs(5)) => {
                     }
        }
    }

    if !pending.is_empty() {
        tracing::warn!(
            "Timeout reached ({}s) with {} requests unfulfilled",
            timeout_secs,
            pending.len()
        );
    }

    Ok(())
}

async fn process(rows: &[BenchRow], db: &str) -> Result<BenchRows> {
    let db = db::Monitor::new(db).await?;
    let mut bench_rows = Vec::new();
    for row in rows {
        let locked_at = db.fetch_locked_at(&row.request_digest).await?;
        let fulfilled_at = db.fetch_fulfilled_at(&row.request_digest).await?;
        let prover = db.fetch_prover(&row.request_digest).await?;
        bench_rows.push(BenchRow {
            request_digest: row.request_digest.clone(),
            request_id: row.request_id.clone(),
            cycle_count: row.cycle_count,
            bid_start: row.bid_start,
            expires_at: row.expires_at,
            locked_at,
            fulfilled_at,
            prover: prover.map(|addr| format!("{addr:x}")),
            effective_latency: locked_at
                .zip(fulfilled_at)
                .map(|(locked, fulfilled)| fulfilled - locked),
            e2e_latency: fulfilled_at.map(|fulfilled_at| fulfilled_at - row.bid_start),
        });
    }
    Ok(BenchRows(bench_rows))
}

async fn fetch_http(url: &Url) -> Result<Vec<u8>> {
    let response = reqwest::get(url.as_str()).await?;
    let status = response.status();
    if !status.is_success() {
        bail!("HTTP request failed with status: {}", status);
    }

    Ok(response.bytes().await?.to_vec())
}

fn now_timestamp() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use std::fs::create_dir_all;

    use alloy::node_bindings::Anvil;
    use boundless_market::contracts::hit_points::default_allowance;
    use boundless_market::storage::StorageProviderConfig;
    use boundless_market_test_utils::{create_test_ctx, LOOP_PATH};
    use broker::{config::Config, Args, Broker};
    use risc0_zkvm::is_dev_mode;
    use tracing_test::traced_test;

    use super::*;

    fn broker_args(
        config_file: PathBuf,
        boundless_market_address: Address,
        set_verifier_address: Address,
        rpc_url: Url,
        private_key: PrivateKeySigner,
    ) -> Args {
        let (bonsai_api_url, bonsai_api_key) = match is_dev_mode() {
            true => (None, None),
            false => (
                Some(
                    Url::parse(
                        &std::env::var("BONSAI_API_URL").expect("BONSAI_API_URL must be set"),
                    )
                    .unwrap(),
                ),
                Some(std::env::var("BONSAI_API_KEY").expect("BONSAI_API_KEY must be set")),
            ),
        };

        Args {
            db_url: "sqlite::memory:".into(),
            config_file,
            boundless_market_address,
            set_verifier_address,
            rpc_url,
            order_stream_url: None,
            private_key,
            bento_api_url: None,
            bonsai_api_key,
            bonsai_api_url,
            deposit_amount: None,
            rpc_retry_max: 3,
            rpc_retry_backoff: 200,
            rpc_retry_cu: 1000,
        }
    }

    async fn new_config_with_min_deadline(min_batch_size: u64, min_deadline: u64) -> NamedTempFile {
        let config_file = tempfile::NamedTempFile::new().expect("Failed to create temp file");
        let mut config = Config::default();
        if !is_dev_mode() {
            config.prover.bonsai_r0_zkvm_ver = Some(risc0_zkvm::VERSION.to_string());
        }
        config.prover.status_poll_ms = 1000;
        config.prover.req_retry_count = 3;
        config.market.mcycle_price = "0.00001".into();
        config.market.mcycle_price_stake_token = "0.0".into();
        config.market.min_deadline = min_deadline;
        config.batcher.min_batch_size = Some(min_batch_size);
        config.write(config_file.path()).await.unwrap();
        config_file
    }

    #[tokio::test]
    #[traced_test]
    #[ignore = "Generates real proofs, slow without dev mode or bonsai"]
    async fn test_bench() {
        let anvil = Anvil::new().spawn();
        let ctx = create_test_ctx(&anvil).await.unwrap();
        ctx.customer_market.deposit(default_allowance()).await.unwrap();
        ctx.prover_market
            .deposit_stake_with_permit(default_allowance(), &ctx.prover_signer)
            .await
            .unwrap();

        // Start a broker
        let config = new_config_with_min_deadline(2, 10).await;
        let args = broker_args(
            config.path().to_path_buf(),
            ctx.boundless_market_address,
            ctx.set_verifier_address,
            anvil.endpoint_url(),
            ctx.prover_signer,
        );
        let broker = Broker::new(args, ctx.prover_provider).await.unwrap();
        let broker_task = tokio::spawn(async move { broker.start_service().await });

        let bench = Bench {
            cycle_count_per_request: 1000,
            requests_count: 2,
            interval: 0,
            timeout: 120,
            lock_timeout: 120,
            min_price_per_mcycle: "0".to_string(),
            max_price_per_mcycle: "0.001".to_string(),
            lockin_stake: "0.0".to_string(),
            ramp_up: 0,
            threads: 1,
        };
        let bench_path = PathBuf::from("out/bench.json");
        if let Some(dir) = bench_path.parent() {
            create_dir_all(dir).unwrap();
        }
        let bench_file = File::create(&bench_path).unwrap();
        serde_json::to_writer_pretty(bench_file, &bench).unwrap();

        let output_path = PathBuf::from("out/output.csv");
        if let Some(dir) = output_path.parent() {
            create_dir_all(dir).unwrap();
        }
        let _output_file = File::create(&output_path).unwrap();

        let args = MainArgs {
            rpc_url: anvil.endpoint_url(),
            order_stream_url: None,
            storage_config: Some(StorageProviderConfig::dev_mode()),
            private_key: ctx.customer_signer.clone(),
            set_verifier_address: ctx.set_verifier_address,
            boundless_market_address: ctx.boundless_market_address,
            program: is_dev_mode().then(|| PathBuf::from(LOOP_PATH)),
            warn_balance_below: None,
            error_balance_below: None,
            indexer_url: None,
            sqlite_path: None,
            bench: bench_path,
            output: Some(output_path.clone()),
            json: false,
            estimate: false,
        };

        run(&args).await.unwrap();

        // Check the output file
        let output = BenchRows::from_file(&output_path).unwrap();
        assert_eq!(output.0.len(), 2);
        for row in &output.0 {
            assert!(row.locked_at.is_some());
            assert!(row.fulfilled_at.is_some());
            assert!(row.prover.is_some());
            assert!(row.effective_latency.is_some());
            assert!(row.e2e_latency.is_some());
        }

        broker_task.abort();
    }
}
