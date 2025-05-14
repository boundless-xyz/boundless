// Copyright (c) 2025 RISC Zero, Inc.
//
// All rights reserved.

use std::{fs::File, path::Path, path::PathBuf, time::Duration};

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
use anyhow::{bail, Result};
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
use risc0_zkvm::{compute_image_id, default_executor, sha::Digestible};
use serde::{Deserialize, Serialize};
use tempfile::NamedTempFile;
use tokio::task::JoinHandle;
use url::Url;

pub mod db;

const LOCK_FULFILL_GAS_UPPER_BOUND: u128 = 100_000_000; // 100M gas

#[derive(Debug, Clone, Deserialize, Serialize)]
/// Parameters for the benchmark
pub struct Bench {
    /// The cycle count per request
    pub cycle_count_per_request: u64,
    /// The number of requests to send
    pub requests_count: u32,
    /// delay between requests in seconds
    ///
    /// If this is set to 0, the requests will be sent as fast as possible.
    pub delay: u64,
    /// Timeout for each request in seconds
    pub timeout: u32,
    /// The lock timeout for each request in seconds
    pub lock_timeout: u32,
}

#[derive(Debug, Serialize)]
pub struct BenchRow {
    pub request_digest: String,
    pub request_id: String,
    pub cycle_count: u64,
    pub bid_start: u64,
    pub expires_at: u64,
    pub fulfilled_at: Option<u64>,
    pub prover: Option<String>,
    pub latency: Option<u64>,
}

impl BenchRow {
    /// Create a new benchmark row
    pub fn new(
        request_digest: String,
        request_id: String,
        cycle_count: u64,
        bid_start: u64,
        expires_at: u64,
    ) -> Self {
        Self {
            request_digest,
            request_id,
            cycle_count,
            bid_start,
            expires_at,
            fulfilled_at: None,
            prover: None,
            latency: None,
        }
    }
}

#[derive(Debug)]
pub struct BenchRows(pub Vec<BenchRow>);

impl BenchRows {
    /// Write the rows out as CSV to `path`.
    pub fn write_csv<P: AsRef<Path>>(&self, path: P) -> Result<()> {
        let file = File::create(path)?;
        let mut wtr = csv::Writer::from_writer(file);

        wtr.write_record(&[
            "request_id",
            "cycle_count",
            "bid_start",
            "expires_at",
            "fulfilled_at",
            "prover",
            "latency",
        ])?;

        for row in &self.0 {
            wtr.serialize(row)?;
        }
        wtr.flush()?;
        Ok(())
    }

    /// Write the rows out as pretty-printed JSON array to `path`.
    pub fn write_json<P: AsRef<Path>>(&self, path: P) -> Result<()> {
        let file = File::create(path)?;
        serde_json::to_writer_pretty(file, &self.0)?;
        Ok(())
    }
}

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
    #[clap(long, env)]
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
    pub indexer_url: Option<String>,
    /// Minimum price per mcycle in ether.
    #[clap(long = "min", value_parser = parse_ether, default_value = "0")]
    pub min_price_per_mcycle: U256,
    /// Maximum price per mcycle in ether.
    #[clap(long = "max", value_parser = parse_ether, default_value = "0.01")]
    pub max_price_per_mcycle: U256,
    /// Lockin stake amount in ether.
    #[clap(short, long, value_parser = parse_ether, default_value = "0.0")]
    pub lockin_stake: U256,
    /// Ramp-up period in seconds.
    ///
    /// The bid price will increase linearly from `min_price` to `max_price` over this period.
    #[clap(long, default_value = "240")] // 240s = ~20 Sepolia blocks
    pub ramp_up: u32,
    /// Program binary file to use as the guest image, given as a path.
    ///
    /// If unspecified, defaults to the included echo guest.
    #[clap(long)]
    pub program: Option<PathBuf>,
    /// Balance threshold at which to log a warning.
    #[clap(long, value_parser = parse_ether, default_value = "1")]
    pub warn_balance_below: Option<U256>,
    /// Balance threshold at which to log an error.
    #[clap(long, value_parser = parse_ether, default_value = "0.1")]
    pub error_balance_below: Option<U256>,
    /// When submitting offchain, auto-deposits an amount in ETH when market balance is below this value.
    ///
    /// This parameter can only be set if order_stream_url is provided.
    #[clap(long, env, value_parser = parse_ether, requires = "order_stream_url")]
    pub auto_deposit: Option<U256>,
    /// The path to the benchmark config file.
    #[clap(long)]
    pub bench: PathBuf,
    /// The path of the output file.
    #[clap(long)]
    pub output: Option<PathBuf>,
}

pub async fn run(args: &MainArgs) -> Result<()> {
    let wallet = EthereumWallet::from(args.private_key.clone());
    let balance_alerts = BalanceAlertConfig {
        watch_address: wallet.default_signer().address(),
        warn_threshold: args.warn_balance_below,
        error_threshold: args.error_balance_below,
    };

    let storage_provider = match &args.storage_config {
        Some(storage_config) => storage_provider_from_config(storage_config).await?,
        None => storage_provider_from_env().await?,
    };

    let boundless_client = ClientBuilder::<BuiltinStorageProvider>::new()
        .with_rpc_url(args.rpc_url.clone())
        .with_storage_provider(Some(storage_provider))
        .with_boundless_market_address(args.boundless_market_address)
        .with_set_verifier_address(args.set_verifier_address)
        .with_order_stream_url(args.order_stream_url.clone())
        .with_private_key(args.private_key.clone())
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
            let url = "https://gateway.pinata.cloud/ipfs/bafkreifpofz3uvc3vq7t2k2o66b2duwvmfab32js7dvn7jldpypwjqisyq";
            (fetch_http(&Url::parse(url)?).await?, Url::parse(url)?)
        }
    };
    let image_id = compute_image_id(&program)?;

    let bench_file = File::open(&args.bench)?;
    let bench: Bench = serde_json::from_reader(bench_file)?;
    let input = bench.cycle_count_per_request;
    let env = InputBuilder::new().write(&(input as u64))?.build_env()?;
    let session_info = default_executor().execute(env.clone().try_into()?, &program)?;
    let journal = session_info.journal;

    let cycles_count = session_info.segments.iter().map(|segment| 1 << segment.po2).sum::<u64>();
    let min_price = args
        .min_price_per_mcycle
        .checked_mul(U256::from(cycles_count))
        .unwrap()
        .div_ceil(U256::from(1_000_000));
    let mcycle_max_price = args
        .max_price_per_mcycle
        .checked_mul(U256::from(cycles_count))
        .unwrap()
        .div_ceil(U256::from(1_000_000));
    let timeout = bench.timeout;
    let lock_timeout = bench.lock_timeout;

    let mut bench_rows = Vec::new();

    let domain = boundless_client.boundless_market.eip712_domain().await?;
    let temp_file = NamedTempFile::new().unwrap();
    let db_url = format!("sqlite:{}", temp_file.path().display());
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

    for i in 0..bench.requests_count {
        // Add to the max price an estimated upper bound on the gas costs.
        // Note that the auction will allow us to pay the lowest price a prover will accept.
        // Add a 10% buffer to the gas costs to account for flucuations after submission.
        let gas_price: u128 = boundless_client.provider().get_gas_price().await?;
        let gas_cost_estimate = (gas_price + (gas_price / 10)) * LOCK_FULFILL_GAS_UPPER_BOUND;
        let max_price = mcycle_max_price + U256::from(gas_cost_estimate);
        tracing::debug!(
            "Setting a max price of {} ether: {} mcycle_price + {} gas_cost_estimate",
            format_units(max_price, "ether")?,
            format_units(mcycle_max_price, "ether")?,
            format_units(gas_cost_estimate, "ether")?,
        );

        tracing::debug!(
            "{} cycles count {} min_price in ether {} max_price in ether",
            cycles_count,
            format_units(min_price, "ether")?,
            format_units(max_price, "ether")?
        );

        let bidding_start = now_timestamp() + 10;
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
                    .with_lock_stake(args.lockin_stake)
                    .with_ramp_up_period(args.ramp_up)
                    .with_timeout(timeout)
                    .with_lock_timeout(lock_timeout),
            )
            .build()?;

        tracing::info!("Request: {:?}", request);

        let submit_offchain = args.order_stream_url.is_some();

        // Check balance and auto-deposit if needed. Only necessary if submitting offchain, since onchain submission automatically deposits
        // in the submitRequest call.
        if submit_offchain {
            if let Some(auto_deposit) = args.auto_deposit {
                let market = boundless_client.boundless_market.clone();
                let caller = boundless_client.caller();
                let balance = market.balance_of(caller).await?;
                tracing::info!(
                    "Caller {} has balance {} ETH on market {}",
                    caller,
                    format_units(balance, "ether")?,
                    args.boundless_market_address
                );
                if balance < auto_deposit {
                    tracing::info!(
                        "Balance {} ETH is below auto-deposit threshold {} ETH, depositing...",
                        format_units(balance, "ether")?,
                        format_units(auto_deposit, "ether")?
                    );
                    market.deposit(auto_deposit).await?;
                    tracing::info!(
                        "Successfully deposited {} ETH",
                        format_units(auto_deposit, "ether")?
                    );
                }
            }
        }

        let (request_id, expires_at) = if submit_offchain {
            boundless_client.submit_request_offchain(&request).await?
        } else {
            boundless_client.submit_request(&request).await?
        };
        request.id = request_id;

        let request_digest = request.eip712_signing_hash(&domain.alloy_struct());
        bench_rows.push(BenchRow::new(
            format!("{request_digest:x}"),
            format!("{request_id:x}"),
            input,
            request.offer.biddingStart,
            expires_at,
        ));

        if submit_offchain {
            tracing::info!(
                "Request 0x{request_id:x} submitted offchain to {} - {}/{}",
                args.order_stream_url.clone().unwrap(),
                i + 1,
                bench.requests_count
            );
        } else {
            tracing::info!(
                "Request 0x{request_id:x} submitted onchain to {} - {}/{}",
                args.boundless_market_address,
                i + 1,
                bench.requests_count
            );
        }

        tokio::time::sleep(Duration::from_secs(bench.delay)).await;
    }

    // wait for the bench to finish
    tracing::info!("Waiting for the bench to finish...");
    tokio::time::sleep(Duration::from_secs(bench.timeout.into())).await;

    // process the rows
    let bench_rows = process(&bench_rows, &indexer_url).await?;
    tracing::info!("Bench finished");
    tracing::info!("Bench rows: {:?}", bench_rows);

    // write the rows to a file
    if let Some(output) = &args.output {
        if output.extension().unwrap_or_default() == "csv" {
            bench_rows.write_csv(output)?;
        } else {
            bench_rows.write_json(output)?;
        }
    } else {
        // write to a new file in the current directory
        let output = PathBuf::from(format!("bench_{}.json", chrono::Utc::now().timestamp()));
        bench_rows.write_json(output)?;
    }

    if let Some(handle) = indexer_handle {
        handle.abort();
    }

    Ok(())
}

async fn process(rows: &[BenchRow], db: &str) -> Result<BenchRows> {
    let db = db::Monitor::new(db).await?;
    let mut bench_rows = Vec::new();
    for row in rows {
        let rd = db.fetch_request_digest(&row.request_digest).await?;
        if rd.is_none() {
            tracing::warn!("Request digest {} not found in the database", row.request_digest);
            continue;
        }
        let fulfilled_at = db.fetch_fulfilled_at(&row.request_digest).await?;
        let prover = db.fetch_prover(&row.request_digest).await?;
        bench_rows.push(BenchRow {
            request_digest: row.request_digest.clone(),
            request_id: row.request_id.clone(),
            cycle_count: row.cycle_count,
            bid_start: row.bid_start,
            expires_at: row.expires_at,
            fulfilled_at,
            prover: prover.map(|addr| format!("{addr:x}")),
            latency: fulfilled_at.map(|fulfilled_at| fulfilled_at - row.bid_start),
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
    use std::fs;

    use alloy::node_bindings::Anvil;
    use boundless_market::contracts::hit_points::default_allowance;
    use boundless_market::storage::StorageProviderConfig;
    use boundless_market_test_utils::create_test_ctx;
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
        let config = new_config_with_min_deadline(2, 30).await;
        let args = broker_args(
            config.path().to_path_buf(),
            ctx.boundless_market_address,
            ctx.set_verifier_address,
            anvil.endpoint_url(),
            ctx.prover_signer.clone(),
        );
        let broker = Broker::new(args, ctx.prover_provider.clone()).await.unwrap();
        let broker_task = tokio::spawn(async move { broker.start_service().await });

        let bench = Bench {
            cycle_count_per_request: 1000,
            requests_count: 2,
            delay: 0,
            timeout: 45,
            lock_timeout: 45,
        };
        let bench_path = PathBuf::from("out/bench.json");
        if let Some(dir) = bench_path.parent() {
            fs::create_dir_all(dir).unwrap();
        }
        let bench_file = File::create(&bench_path).unwrap();
        serde_json::to_writer_pretty(bench_file, &bench).unwrap();

        let output_path = PathBuf::from("out/output.json");
        if let Some(dir) = output_path.parent() {
            fs::create_dir_all(dir).unwrap();
        }
        let output_file = File::create(&output_path).unwrap();

        let args = MainArgs {
            rpc_url: anvil.endpoint_url(),
            order_stream_url: None,
            storage_config: Some(StorageProviderConfig::dev_mode()),
            private_key: ctx.customer_signer.clone(),
            set_verifier_address: ctx.set_verifier_address,
            boundless_market_address: ctx.boundless_market_address,
            min_price_per_mcycle: parse_ether("0.001").unwrap(),
            max_price_per_mcycle: parse_ether("0.002").unwrap(),
            lockin_stake: parse_ether("0.0").unwrap(),
            ramp_up: 0,
            program: None,
            warn_balance_below: None,
            error_balance_below: None,
            auto_deposit: None,
            indexer_url: None,
            bench: bench_path,
            output: Some(output_path),
        };

        let client_addr = ctx.customer_signer.address();
        tracing::info!("Client address: {:x}", client_addr);
        let client_balance = ctx.customer_provider.get_balance(client_addr).await.unwrap();

        let addr = ctx.prover_signer.address();
        tracing::info!("Prover address: {:x}", addr);
        let balance = ctx.prover_provider.get_balance(addr).await.unwrap();

        run(&args).await.unwrap();

        tracing::info!(
            "Client balance before bench: {}",
            format_units(client_balance, "ether").unwrap()
        );
        let client_balance = ctx.customer_provider.get_balance(client_addr).await.unwrap();
        tracing::info!(
            "Client balance after bench: {}",
            format_units(client_balance, "ether").unwrap()
        );

        tracing::info!("Prover balance before bench: {}", format_units(balance, "ether").unwrap());
        let balance = ctx.prover_provider.get_balance(addr.clone()).await.unwrap();
        tracing::info!("Prover balance after bench: {}", format_units(balance, "ether").unwrap());

        let balance_of_stake = ctx.prover_market.balance_of_stake(addr).await.unwrap();
        tracing::info!(
            "Prover balance of stake after bench: {}",
            format_units(balance_of_stake, "ether").unwrap()
        );

        broker_task.abort();
    }
}
