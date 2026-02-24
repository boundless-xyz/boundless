// Copyright 2026 Boundless Foundation, Inc.
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

mod derivation;
mod indexer;
mod rotation;

use std::{collections::HashSet, path::PathBuf, time::Duration};

use alloy::{
    eips::BlockNumberOrTag,
    network::EthereumWallet,
    primitives::{
        utils::{format_units, parse_ether},
        U256,
    },
    providers::{DynProvider, Provider},
    signers::local::PrivateKeySigner,
};
use anyhow::{Context, Result};
use boundless_market::{
    balance_alerts_layer::BalanceAlertConfig,
    client::{Client, FundingMode},
    deployments::Deployment,
    indexer_client::IndexerClient,
    input::GuestEnv,
    request_builder::StandardRequestBuilder,
    storage::{HttpDownloader, StandardDownloader, StorageDownloader},
    StandardUploader, StorageUploaderConfig,
};
use clap::Parser;
use indexer::{is_safe_to_withdraw, next_safe_at, RequestorIndexer};
use rand::Rng;
use risc0_zkvm::Journal;
use tracing_subscriber::fmt::format::FmtSpan;
use url::Url;

type OrderGeneratorClient = Client<
    DynProvider,
    StandardUploader,
    StandardDownloader,
    StandardRequestBuilder<DynProvider, StandardUploader, StandardDownloader>,
    PrivateKeySigner,
>;

/// Arguments of the order generator.
#[derive(Parser, Debug)]
#[clap(author, version, about, long_about = None)]
struct MainArgs {
    /// URL of the Ethereum RPC endpoint.
    #[clap(short, long, env)]
    rpc_url: Option<Url>,
    /// Additional RPC URLs for automatic failover.
    #[clap(long, env = "RPC_URLS", value_delimiter = ',')]
    rpc_urls: Option<Vec<Url>>,
    /// Private key used to sign and submit requests (or funding source when rotation enabled).
    /// Not required when using --mnemonic with --derive-rotation-keys.
    #[clap(long, env)]
    private_key: Option<PrivateKeySigner>,
    /// BIP-39 mnemonic phrase for key derivation. Use with --derive-rotation-keys for standard
    /// BIP-39/BIP-32 derivation (keys match MetaMask, Ledger, etc.). Pass via --mnemonic or MNEMONIC env.
    #[clap(long, env, requires = "derive_rotation_keys", hide_env_values = true)]
    mnemonic: Option<String>,
    /// Derive N rotation keys from mnemonic using BIP-39/BIP-32 (m/44'/60'/0'/0/{0..N}).
    /// Index 0 = funding key; indices 1..N = rotation keys. Requires N >= 2 and --mnemonic.
    #[clap(long, env = "DERIVE_ROTATION_KEYS", requires = "mnemonic")]
    derive_rotation_keys: Option<usize>,
    /// Interval in seconds between address rotations (e.g., 86400 = daily).
    #[clap(long, default_value = "86400")]
    address_rotation_interval: u64,
    /// Transaction timeout in seconds.
    #[clap(long, default_value = "45")]
    tx_timeout: u64,
    /// Number of retry attempts for failed request submissions (transient confirmation errors).
    #[clap(long, env = "SUBMIT_RETRY_ATTEMPTS", default_value = "3")]
    submit_retry_attempts: u32,
    /// Delay in seconds between submission retries.
    #[clap(long, env = "SUBMIT_RETRY_DELAY_SECS", default_value = "2")]
    submit_retry_delay_secs: u64,
    /// Market balance threshold for top-up (rotation mode). Must match FundingMode::BelowThreshold.
    #[clap(long, env = "TOP_UP_MARKET_THRESHOLD", value_parser = parse_ether, default_value = "0.01")]
    top_up_market_threshold: U256,
    /// Native ETH threshold for top-up (covers deposit + submit tx gas).
    #[clap(long, env = "TOP_UP_NATIVE_THRESHOLD", value_parser = parse_ether, default_value = "0.05")]
    top_up_native_threshold: U256,
    /// Timeout in seconds for withdrawal/sweep transactions (rotation). Default 120.
    #[clap(long, env = "WITHDRAWAL_TX_TIMEOUT", default_value = "120")]
    withdrawal_tx_timeout: u64,
    /// Retry attempts for the sweep transfer during rotation when confirmation times out.
    #[clap(long, env = "WITHDRAWAL_SWEEP_RETRIES", default_value = "3")]
    withdrawal_sweep_retries: u32,
    /// When submitting offchain, auto-deposits an amount in ETH when market balance is below this value.
    ///
    /// This parameter can only be set if order_stream_url is provided.
    #[clap(long, env, value_parser = parse_ether)]
    auto_deposit: Option<U256>,
    /// Interval in seconds between requests.
    #[clap(short, long, default_value = "60")]
    interval: u64,
    /// Optional number of requests to submit.
    ///
    /// If unspecified, the loop will run indefinitely.
    #[clap(short, long)]
    count: Option<u64>,
    /// Minimum price per mcycle in ether.
    #[clap(long = "min", value_parser = parse_ether, default_value = "0.001")]
    min_price_per_mcycle: U256,
    /// Maximum price per mcycle in ether. If set, overrides the offer's maxPrice
    /// with max_price_per_mcycle * mcycles. Overridden by max_price_cap if both are set.
    #[clap(long = "max", value_parser = parse_ether)]
    max_price_per_mcycle: Option<U256>,
    /// Lockin stake amount in ether.
    #[clap(short, long, default_value = "0")]
    lock_collateral_raw: U256,
    /// Number of seconds, from the current time, before the auction period starts.
    /// If not provided, will be calculated based on cycle count assuming 5 MHz prove rate.
    #[clap(long)]
    bidding_start_delay: Option<u64>,
    /// Ramp-up period in seconds.
    ///
    /// The bid price will increase linearly from `min_price` to `max_price` over this period.
    #[clap(long, default_value = "240")] // 240s = ~20 Sepolia blocks
    ramp_up: u32,
    /// Number of seconds before the request lock-in expires.
    #[clap(long, default_value = "900")]
    lock_timeout: u32,
    /// Number of seconds before the request expires.
    #[clap(long, default_value = "1800")]
    timeout: u32,
    /// Additional time in seconds to add to the timeout for each 1M cycles.
    #[clap(long, default_value = "20")]
    seconds_per_mcycle: u32,
    /// Additional time in seconds to add to the ramp-up period for each 1M cycles.
    #[clap(long, default_value = "20")]
    ramp_up_seconds_per_mcycle: u32,
    /// Execution rate in kHz for calculating bidding start delays.
    /// Default is 2000 kHz (2 MHz).
    #[clap(long, default_value = "2000", env)]
    exec_rate_khz: u64,
    /// Program binary file to use as the guest image, given as a path.
    ///
    /// If unspecified, defaults to the included loop guest.
    #[clap(long)]
    program: Option<PathBuf>,
    /// The cycle count to drive the loop.
    ///
    /// If unspecified, defaults to a random value between 10_000_000 and 1_000_000_000
    /// with a step of 1_000_000.
    #[clap(long, env = "CYCLE_COUNT")]
    input: Option<u64>,
    /// The maximum cycle count to drive the loop.
    #[clap(long, env = "CYCLE_COUNT_MAX", conflicts_with_all = ["input", "program"])]
    input_max_mcycles: Option<u64>,
    /// Balance threshold at which to log a warning.
    #[clap(long, value_parser = parse_ether, default_value = "1")]
    warn_balance_below: Option<U256>,
    /// Balance threshold at which to log an error.
    #[clap(long, value_parser = parse_ether, default_value = "0.1")]
    error_balance_below: Option<U256>,

    /// Boundless Market deployment configuration
    #[clap(flatten, next_help_heading = "Boundless Market Deployment")]
    deployment: Option<Deployment>,

    /// Submit requests offchain.
    #[clap(long, default_value = "false")]
    submit_offchain: bool,

    /// Configuration for the uploader used for programs and inputs.
    #[clap(flatten, next_help_heading = "Storage Uploader")]
    storage_config: StorageUploaderConfig,

    /// Whether to use the Zeth guest.
    #[clap(long, default_value = "false")]
    use_zeth: bool,

    /// Maximum total price cap for the request in ether.
    /// If set and the offer's maxPrice exceeds this value, caps it with a warning.
    #[clap(long, value_parser = parse_ether)]
    max_price_cap: Option<U256>,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_ansi(false)
        .with_span_events(FmtSpan::CLOSE)
        .json()
        .init();

    let args = MainArgs::parse();

    // NOTE: Using a separate `run` function to facilitate testing below.
    let result = run(&args).await;
    if let Err(e) = result {
        tracing::error!("FATAL: {:?}", e);
    }

    Ok(())
}

/// Resolve funding signer and rotation keys. Returns (funding, rotation_keys) when rotation enabled.
fn resolve_rotation_keys(
    args: &MainArgs,
) -> Result<Option<(PrivateKeySigner, Vec<PrivateKeySigner>)>> {
    match (args.derive_rotation_keys, &args.mnemonic) {
        (Some(n), Some(phrase)) => {
            if n < 2 {
                anyhow::bail!("--derive-rotation-keys must be >= 2, got {n}");
            }
            let count = n + 1; // funding (0) + rotation keys (1..n)
            let derived = derivation::derive_keys(phrase, count)
                .context("Failed to derive rotation keys from mnemonic")?;
            let funding = derived[0].clone();
            let rotation = derived[1..].to_vec();
            Ok(Some((funding, rotation)))
        }
        _ => Ok(None),
    }
}

async fn run(args: &MainArgs) -> Result<()> {
    let rotation = resolve_rotation_keys(args)?;
    let (funding_signer, rotation_keys) = match rotation {
        Some((f, r)) => {
            // Use private_key as funding source when provided (e.g. for tests with pre-funded account)
            let funding = args.private_key.as_ref().unwrap_or(&f);
            (funding.clone(), r)
        }
        _ => {
            let pk = args.private_key.as_ref().ok_or_else(|| {
                anyhow::anyhow!("--private-key or --mnemonic with --derive-rotation-keys required")
            })?;
            return run_single_key(args, pk).await;
        }
    };

    run_with_rotation(args, &funding_signer, &rotation_keys).await
}

async fn run_single_key(args: &MainArgs, signer: &PrivateKeySigner) -> Result<()> {
    let wallet = EthereumWallet::from(signer.clone());
    let balance_alerts = BalanceAlertConfig {
        watch_address: wallet.default_signer().address(),
        warn_threshold: args.warn_balance_below,
        error_threshold: args.error_balance_below,
    };

    let mut client_builder = Client::builder();
    if let Some(rpc_url) = &args.rpc_url {
        client_builder = client_builder.with_rpc_url(rpc_url.clone());
    }
    if let Some(rpc_urls) = &args.rpc_urls {
        client_builder = client_builder.with_rpc_urls(rpc_urls.clone());
    }

    let client = client_builder
        .with_uploader_config(&args.storage_config)
        .await?
        .with_deployment(args.deployment.clone())
        .with_private_key(signer.clone())
        .with_balance_alerts(balance_alerts)
        .with_timeout(Some(Duration::from_secs(args.tx_timeout)))
        .with_funding_mode(FundingMode::BelowThreshold(args.top_up_market_threshold))
        .build()
        .await?;

    let ipfs_gateway = args
        .storage_config
        .ipfs_gateway_url
        .clone()
        .unwrap_or(Url::parse("https://gateway.pinata.cloud").unwrap());
    let (program, program_url, input) =
        load_program_and_input(args, &client, &ipfs_gateway).await?;

    // Zeth mode requires input; validate early to avoid a silent per-iteration failure.
    if args.use_zeth && input.is_none() {
        return Err(anyhow::anyhow!("Zeth input is not provided"));
    }

    let mut i = 0u64;
    loop {
        if let Some(count) = args.count {
            if i >= count {
                break;
            }
        }
        if let Err(e) =
            submit_request_with_retry(args, &client, &program, &program_url, input.as_deref())
                .await
                .map(|_| ())
        {
            let error_msg = format!("{e:?}");
            if error_msg.contains("Transaction confirmation error") {
                tracing::error!("[B-OG-CONF] Transaction confirmation error: {e:?}");
            } else {
                tracing::error!("Request failed: {e:?}");
            }
        }
        i += 1;
        tokio::time::sleep(Duration::from_secs(args.interval)).await;
    }

    Ok(())
}

async fn run_with_rotation(
    args: &MainArgs,
    funding_signer: &PrivateKeySigner,
    private_keys: &[PrivateKeySigner],
) -> Result<()> {
    let n_keys = private_keys.len();

    let funding_client = build_client_for_signer(args, funding_signer).await?;
    // Build rotation clients on demand to avoid bursting the price oracle at startup.
    let mut rotation_clients: Vec<Option<OrderGeneratorClient>> =
        (0..n_keys).map(|_| None).collect();

    // Build the indexer client used to check request status before sweeping old keys.
    // Prefer the URL from the deployment config; fall back to chain-ID lookup.
    let indexer =
        if let Some(url_str) = args.deployment.as_ref().and_then(|d| d.indexer_url.as_deref()) {
            let url = url_str.parse::<Url>().context("Invalid indexer URL in deployment config")?;
            IndexerClient::new(url).context("Failed to build indexer client")?
        } else {
            let chain_id = funding_client
                .provider()
                .get_chain_id()
                .await
                .context("Failed to get chain ID for indexer client")?;
            IndexerClient::new_from_chain_id(chain_id).context("Failed to build indexer client")?
        };

    let gas_reserve = parse_ether(GAS_RESERVE).unwrap();

    let ipfs_gateway = args
        .storage_config
        .ipfs_gateway_url
        .clone()
        .unwrap_or(Url::parse("https://gateway.pinata.cloud").unwrap());
    let (program, program_url, input) =
        load_program_and_input(args, &funding_client, &ipfs_gateway).await?;

    let mut i = 0u64;
    let mut success_count = 0u64;
    let mut last_error: Option<anyhow::Error> = None;
    let poll_interval = Duration::from_secs(args.interval);
    // Track keys that have been fully swept this rotation cycle so we skip their
    // balance RPC calls on subsequent iterations. Cleared when a key becomes desired
    // again (i.e. when keys cycle back around after n_keys * interval).
    let mut swept_keys: HashSet<usize> = HashSet::new();

    loop {
        if let Some(count) = args.count {
            if i >= count {
                break;
            }
        }

        let now = get_block_timestamp(&funding_client.provider()).await.unwrap_or_else(|e| {
            tracing::warn!("Failed to get block timestamp, using system time: {e:?}");
            rotation::now_secs()
        });
        let desired = rotation::desired_index(now, args.address_rotation_interval, n_keys);
        // When a key becomes desired again it accumulates new balance/requests,
        // so drop any prior "already swept" marker for it.
        swept_keys.remove(&desired);

        // For every non-desired key: if it has funds worth sweeping, query the indexer
        // to check whether all its requests have settled. Sweep immediately if safe,
        // otherwise log the expected wait time and retry next iteration.
        for old_idx in 0..n_keys {
            if old_idx == desired {
                continue;
            }
            if swept_keys.contains(&old_idx) {
                continue; // already swept this rotation cycle — skip balance RPC calls
            }
            let old_addr = private_keys[old_idx].address();
            let market_bal =
                funding_client.boundless_market.balance_of(old_addr).await.unwrap_or(U256::ZERO);
            let native_bal =
                funding_client.provider().get_balance(old_addr).await.unwrap_or(U256::ZERO);
            if market_bal == U256::ZERO && native_bal <= gas_reserve {
                continue; // nothing worth sweeping
            }
            match indexer.list_requests(old_addr).await {
                Ok(requests) => {
                    if is_safe_to_withdraw(&requests, now) {
                        tracing::info!(
                            "Sweeping rotated key {old_idx} ({old_addr}): all requests settled"
                        );
                        if rotation_clients[old_idx].is_none() {
                            rotation_clients[old_idx] =
                                Some(build_client_for_signer(args, &private_keys[old_idx]).await?);
                        }
                        perform_rotation_withdrawal(
                            args,
                            &funding_client,
                            rotation_clients[old_idx].as_ref().unwrap(),
                        )
                        .await?;
                        swept_keys.insert(old_idx);
                    } else {
                        let safe_at = next_safe_at(&requests);
                        tracing::info!(
                            "Key {old_idx} ({old_addr}) has unsettled requests; \
                             sweep deferred ~{}s",
                            safe_at.saturating_sub(now),
                        );
                    }
                }
                Err(e) => {
                    tracing::warn!(
                        "Indexer unavailable for {old_addr}, skipping sweep this iteration: {e:?}"
                    );
                }
            }
        }

        if rotation_clients[desired].is_none() {
            rotation_clients[desired] =
                Some(build_client_for_signer(args, &private_keys[desired]).await?);
        }
        let client = rotation_clients[desired].as_ref().unwrap();
        top_up_from_funding_source(args, &funding_client, client, private_keys[desired].address())
            .await?;

        let result =
            submit_request_with_retry(args, client, &program, &program_url, input.as_deref()).await;

        match result {
            Ok(_) => {
                success_count += 1;
            }
            Err(e) => {
                let error_msg = format!("{e:?}");
                last_error = Some(e);
                if error_msg.contains("Transaction confirmation error") {
                    tracing::error!("[B-OG-CONF] Transaction confirmation error: {error_msg}");
                } else {
                    tracing::error!("Request failed: {error_msg}");
                }
            }
        }

        i += 1;
        tokio::time::sleep(poll_interval).await;
    }

    if let Some(count) = args.count {
        if count > 0 && success_count == 0 {
            return Err(last_error.unwrap_or_else(|| {
                anyhow::anyhow!("All {count} requests failed (no successful submissions)")
            }));
        }
    }

    Ok(())
}

async fn submit_request_with_retry(
    args: &MainArgs,
    client: &OrderGeneratorClient,
    program: &[u8],
    program_url: &Url,
    zeth_input: Option<&[u8]>,
) -> Result<(U256, u64, u64)> {
    let mut result = match args.use_zeth {
        true => {
            let input = zeth_input.ok_or_else(|| anyhow::anyhow!("Zeth input is not provided"))?;
            handle_zeth_request(args, client, program, program_url, input).await
        }
        false => handle_loop_request(args, client, program, program_url).await,
    };

    let max_retries = args.submit_retry_attempts;
    let retry_delay = Duration::from_secs(args.submit_retry_delay_secs);
    for attempt in 1..=max_retries {
        if result.is_ok() {
            return result;
        }
        tracing::warn!(
            "Submission failed (attempt {attempt}/{max_retries}), retrying in {}s: {:?}",
            args.submit_retry_delay_secs,
            result.as_ref().unwrap_err()
        );
        tokio::time::sleep(retry_delay).await;
        result = match args.use_zeth {
            true => {
                let input =
                    zeth_input.ok_or_else(|| anyhow::anyhow!("Zeth input is not provided"))?;
                handle_zeth_request(args, client, program, program_url, input).await
            }
            false => handle_loop_request(args, client, program, program_url).await,
        };
    }
    result
}

async fn load_program_and_input(
    args: &MainArgs,
    client: &OrderGeneratorClient,
    ipfs_gateway: &Url,
) -> Result<(Vec<u8>, Url, Option<Vec<u8>>)> {
    match args.program.as_ref().map(std::fs::read).transpose()? {
        Some(program) => {
            let program_url = client.upload_program(&program).await?;
            tracing::info!("Uploaded program to {}", program_url);
            Ok((program, program_url, None))
        }
        None => {
            let (program_url, input) = match args.use_zeth {
                true => {
                    let program_url = ipfs_gateway
                        .join("/ipfs/bafybeidxu26oi63himx2hn5vbhmwc3t6rabqmkpl4i7nl7daucvcvq6cge")
                        .unwrap();
                    let input = HttpDownloader::default()
                        .download_url(
                            ipfs_gateway
                                .join("/ipfs/bafybeiac26qnu67cqlklukxdtr5cvovbdss4dx64ywi2yx53mw3ykwmufe")
                                .unwrap(),
                        )
                        .await?;
                    (program_url, Some(input))
                }
                false => (
                    ipfs_gateway
                        .join("/ipfs/bafkreicmwk3xlxbozbp5h63xyywocc7dltt376hn4mnmhk7ojqdcbrkqzi")
                        .unwrap(),
                    None,
                ),
            };
            let program = HttpDownloader::default().download_url(program_url.clone()).await?;
            Ok((program, program_url, input))
        }
    }
}

async fn build_client_for_signer(
    args: &MainArgs,
    signer: &PrivateKeySigner,
) -> Result<OrderGeneratorClient> {
    let mut client_builder = Client::builder();
    if let Some(rpc_url) = &args.rpc_url {
        client_builder = client_builder.with_rpc_url(rpc_url.clone());
    }
    if let Some(rpc_urls) = &args.rpc_urls {
        client_builder = client_builder.with_rpc_urls(rpc_urls.clone());
    }

    client_builder
        .with_uploader_config(&args.storage_config)
        .await?
        .with_deployment(args.deployment.clone())
        .with_private_key(signer.clone())
        .with_timeout(Some(Duration::from_secs(args.tx_timeout)))
        .with_funding_mode(FundingMode::BelowThreshold(args.top_up_market_threshold))
        .build()
        .await
}

/// Reserve left on funding/old address for gas (withdraw + transfer txs).
const GAS_RESERVE: &str = "0.001";

/// Conservative upper bound on request lifetime, saved to state *before* each
/// submission attempt. If the service crashes between tx confirmation and the
/// post-submit state save, the loaded `max_expires_at` still covers the live
/// request, preventing a premature rotation/withdrawal on restart.
async fn top_up_from_funding_source(
    args: &MainArgs,
    funding_client: &OrderGeneratorClient,
    rotation_client: &OrderGeneratorClient,
    to_address: alloy::primitives::Address,
) -> Result<()> {
    let market_threshold = args.top_up_market_threshold;
    let native_threshold = args.top_up_native_threshold;
    let market = rotation_client.boundless_market.clone();
    let market_balance =
        market.balance_of(to_address).await.context("failed to get market balance")?;
    let native_balance = rotation_client
        .provider()
        .get_balance(to_address)
        .await
        .context("failed to get native balance")?;
    if market_balance >= market_threshold && native_balance >= native_threshold {
        return Ok(());
    }

    // The rotation address needs native ETH to cover:
    // - Market deposit shortfall (FundingMode::BelowThreshold handles the deposit itself)
    // - Native ETH for gas (submit tx, deposit tx, etc.)
    let market_shortfall = market_threshold.saturating_sub(market_balance);
    let native_shortfall = native_threshold.saturating_sub(native_balance);
    let needed = market_shortfall.saturating_add(native_shortfall);
    if needed == U256::ZERO {
        return Ok(());
    }

    let funding_balance = funding_client
        .provider()
        .get_balance(funding_client.caller())
        .await
        .context("failed to get funding source balance")?;
    let gas_reserve = parse_ether(GAS_RESERVE).unwrap();
    let available = funding_balance.saturating_sub(gas_reserve);
    if available == U256::ZERO {
        tracing::warn!(
            "Funding source {} has insufficient balance to top up {}",
            funding_client.caller(),
            to_address
        );
        return Ok(());
    }
    let top_up = needed.min(available);
    funding_client
        .transfer_value(to_address, top_up)
        .await
        .context("failed to transfer from funding source")?;
    tracing::info!(
        "Topped up {} with {} ETH from funding source",
        to_address,
        format_units(top_up, "ether")?
    );
    Ok(())
}

async fn get_block_timestamp(provider: &impl Provider) -> Result<u64> {
    let block = provider
        .get_block_by_number(BlockNumberOrTag::Latest)
        .await
        .context("failed to get block")?
        .context("block not found")?;
    Ok(block.header.timestamp)
}

/// Withdraw the market balance from `old_client` and sweep remaining native ETH
/// back to the funding address. Called once at rotation time after `wait_for_expiry`.
/// Uses a longer timeout and retries for the sweep to avoid rotation failure on slow chains.
async fn perform_rotation_withdrawal(
    args: &MainArgs,
    funding_client: &OrderGeneratorClient,
    old_client: &OrderGeneratorClient,
) -> Result<()> {
    let old_address = old_client.caller();
    let funding_address = funding_client.caller();

    let market = old_client.boundless_market.clone();
    let market_balance =
        market.balance_of(old_address).await.context("failed to get market balance")?;
    let gas_reserve = parse_ether(GAS_RESERVE).unwrap();
    let native_balance = old_client
        .provider()
        .get_balance(old_address)
        .await
        .context("failed to get native balance")?;

    // Withdraw market balance if present and we have enough gas.
    if market_balance > U256::ZERO {
        if native_balance < gas_reserve {
            tracing::warn!(
                "Insufficient native ETH ({}) on {old_address} for market withdrawal gas, skipping",
                format_units(native_balance, "ether").unwrap_or_default(),
            );
        } else {
            market.withdraw(market_balance).await.context("failed to withdraw from market")?;
        }
    } else {
        tracing::debug!("No market balance to withdraw from {old_address}");
    }

    // Sweep all native ETH (minus gas reserve for this transfer) back to funding.
    // This captures both the withdrawn market balance and any leftover gas money,
    // including funds stranded on the rotation address when market balance was zero.
    let post_balance = old_client
        .provider()
        .get_balance(old_address)
        .await
        .context("failed to get post-withdraw native balance")?;
    let transfer_amount = post_balance.saturating_sub(gas_reserve);
    if transfer_amount > U256::ZERO {
        let sweep_timeout = Duration::from_secs(args.withdrawal_tx_timeout);
        let max_attempts = args.withdrawal_sweep_retries;
        let mut last_err = None;
        for attempt in 1..=max_attempts {
            match old_client
                .transfer_value_with_timeout(funding_address, transfer_amount, sweep_timeout)
                .await
            {
                Ok(()) => {
                    tracing::info!(
                        "Swept {} ETH from {old_address} to funding {funding_address}",
                        format_units(transfer_amount, "ether").unwrap_or_default(),
                    );
                    last_err = None;
                    break;
                }
                Err(e) => {
                    last_err = Some(e);
                    if attempt < max_attempts {
                        tracing::warn!(
                            "Sweep attempt {attempt}/{max_attempts} failed (timeout {}s), retrying: {:?}",
                            args.withdrawal_tx_timeout,
                            last_err,
                        );
                        tokio::time::sleep(Duration::from_secs(2)).await;
                    }
                }
            }
        }
        if let Some(e) = last_err {
            tracing::warn!(
                "Failed to sweep {} ETH from {old_address} to funding {funding_address} after {max_attempts} attempts: {e:?}. Rotation will continue.",
                format_units(transfer_amount, "ether").unwrap_or_default(),
            );
        }
    }
    tracing::info!(
        "Rotated from {old_address}: withdrew {} ETH from market",
        format_units(market_balance, "ether")?,
    );
    Ok(())
}

async fn handle_loop_request(
    args: &MainArgs,
    client: &OrderGeneratorClient,
    program: &[u8],
    program_url: &url::Url,
) -> Result<(U256, u64, u64)> {
    let mut rng = rand::rng();
    let nonce: u64 = rng.random();
    let input = match args.input {
        Some(input) => input,
        None => {
            // Generate a random input.
            let max = args.input_max_mcycles.unwrap_or(1000);
            let input: u64 = rand::rng().random_range(10..=max) << 20;
            tracing::debug!("Generated random cycle count: {}", input);
            input
        }
    };
    let env = GuestEnv::builder().write(&(input as u64))?.write(&nonce)?.build_env();

    let m_cycles = input >> 20;

    // Provide journal and cycles in order to skip preflighting, allowing us to send requests faster.
    let journal = Journal::new([input.to_le_bytes(), nonce.to_le_bytes()].concat());

    let request = client
        .new_request()
        .with_program(program.to_vec())
        .with_program_url(program_url.clone())?
        .with_env(env)
        .with_cycles(input)
        .with_journal(journal);

    // Build the request, including preflight, and assigned the remaining fields.
    let mut request = client.build_request(request).await?;

    if let Some(max_price_per_mcycle) = args.max_price_per_mcycle {
        let per_mcycle_max = max_price_per_mcycle * U256::from(m_cycles);
        tracing::info!(
            "Applying max_price_per_mcycle: {} ether/mcycle * {} mcycles = {} ether (build produced {} ether)",
            format_units(max_price_per_mcycle, "ether")?,
            m_cycles,
            format_units(per_mcycle_max, "ether")?,
            format_units(request.offer.maxPrice, "ether")?,
        );
        request.offer.maxPrice = per_mcycle_max;
    }

    if let Some(max_price_cap) = args.max_price_cap {
        if request.offer.maxPrice > max_price_cap {
            tracing::warn!(
                "maxPrice {} ether exceeds max_price_cap {} ether, capping",
                format_units(request.offer.maxPrice, "ether")?,
                format_units(max_price_cap, "ether")?
            );
            request.offer.maxPrice = max_price_cap;
        }
    }

    tracing::info!("Request: {:?}", request);

    tracing::info!(
        "{} Mcycles count {} min_price in ether {} max_price in ether",
        m_cycles,
        format_units(request.offer.minPrice, "ether")?,
        format_units(request.offer.maxPrice, "ether")?
    );

    ensure_auto_deposit(args, client).await?;

    let (request_id, expires_at) = if args.submit_offchain {
        client.submit_request_offchain(&request).await?
    } else {
        client.submit_request_onchain(&request).await?
    };

    if args.submit_offchain {
        tracing::info!(
            "Request 0x{request_id:x} submitted offchain to {}",
            client.deployment.order_stream_url.clone().unwrap()
        );
    } else {
        tracing::info!(
            "Request 0x{request_id:x} submitted onchain to {}",
            client.deployment.boundless_market_address,
        );
    }
    let ramp_up_end = request.offer.rampUpStart + request.offer.rampUpPeriod as u64;
    Ok((request_id, expires_at, ramp_up_end))
}

async fn ensure_auto_deposit(args: &MainArgs, client: &OrderGeneratorClient) -> Result<()> {
    let Some(auto_deposit) = args.auto_deposit else {
        return Ok(());
    };
    let market = client.boundless_market.clone();
    let caller = client.caller();
    let balance = market.balance_of(caller).await?;
    tracing::info!(
        "Caller {} has balance {} ETH on market {}. Auto-deposit threshold is {} ETH",
        caller,
        format_units(balance, "ether")?,
        client.deployment.boundless_market_address,
        format_units(auto_deposit, "ether")?
    );
    if balance < auto_deposit {
        tracing::info!(
            "Balance {} ETH is below auto-deposit threshold {} ETH, depositing...",
            format_units(balance, "ether")?,
            format_units(auto_deposit, "ether")?
        );
        if let Err(e) = market.deposit(auto_deposit).await {
            tracing::warn!("Failed to auto deposit ETH: {e:?}");
        } else {
            tracing::info!("Successfully deposited {} ETH", format_units(auto_deposit, "ether")?);
        }
    }
    Ok(())
}

async fn handle_zeth_request(
    args: &MainArgs,
    client: &OrderGeneratorClient,
    program: &[u8],
    program_url: &url::Url,
    input: &[u8],
) -> Result<(U256, u64, u64)> {
    let mut rng = rand::rng();
    let nonce: u32 = rng.random_range(0..=u32::MAX);
    let max_cycles = args.input_max_mcycles.unwrap_or(3000) << 20;
    let max_iterations = max_cycles.div_ceil(1000000000); // ~1B cycles per iteration
    let iterations: u32 = rng.random_range(1..=max_iterations as u32);
    tracing::info!("Iterations: {}", iterations);
    let env = GuestEnv::builder().write_slice(input).write(&iterations)?.write(&nonce)?.build_env();
    let request = client
        .new_request()
        .with_program(program.to_vec())
        .with_program_url(program_url.clone())?
        .with_env(env);

    // Build the request, including preflight, and assigned the remaining fields.
    let mut request = client.build_request(request).await?;

    let m_cycles = max_cycles >> 20;
    if let Some(max_price_per_mcycle) = args.max_price_per_mcycle {
        let per_mcycle_max = max_price_per_mcycle * U256::from(m_cycles);
        tracing::info!(
            "Applying max_price_per_mcycle: {} ether/mcycle * {} mcycles = {} ether (build produced {} ether)",
            format_units(max_price_per_mcycle, "ether")?,
            m_cycles,
            format_units(per_mcycle_max, "ether")?,
            format_units(request.offer.maxPrice, "ether")?,
        );
        request.offer.maxPrice = per_mcycle_max;
    }

    if let Some(max_price_cap) = args.max_price_cap {
        if request.offer.maxPrice > max_price_cap {
            tracing::warn!(
                "maxPrice {} ether exceeds max_price_cap {} ether, capping",
                format_units(request.offer.maxPrice, "ether")?,
                format_units(max_price_cap, "ether")?
            );
            request.offer.maxPrice = max_price_cap;
        }
    }

    tracing::info!("Request: {:?}", request);

    tracing::info!(
        "{} min_price in ether {} max_price in ether",
        format_units(request.offer.minPrice, "ether")?,
        format_units(request.offer.maxPrice, "ether")?
    );

    ensure_auto_deposit(args, client).await?;

    let (request_id, expires_at) = if args.submit_offchain {
        client.submit_request_offchain(&request).await?
    } else {
        client.submit_request_onchain(&request).await?
    };

    if args.submit_offchain {
        tracing::info!(
            "Request 0x{request_id:x} submitted offchain to {}",
            client.deployment.order_stream_url.clone().unwrap()
        );
    } else {
        tracing::info!(
            "Request 0x{request_id:x} submitted onchain to {}",
            client.deployment.boundless_market_address,
        );
    }
    let ramp_up_end = request.offer.rampUpStart + request.offer.rampUpPeriod as u64;
    Ok((request_id, expires_at, ramp_up_end))
}

#[cfg(test)]
mod tests {
    use alloy::{
        node_bindings::Anvil,
        providers::{ext::AnvilApi, Provider},
        rpc::types::Filter,
        sol_types::SolEvent,
    };
    use boundless_market::contracts::IBoundlessMarket;
    use boundless_test_utils::{guests::LOOP_PATH, market::create_test_ctx};

    use super::*;

    const BIP39_TEST_MNEMONIC: &str =
        "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";

    fn base_test_args() -> MainArgs {
        MainArgs {
            rpc_url: None,
            rpc_urls: None,
            storage_config: StorageUploaderConfig::dev_mode(),
            private_key: None,
            mnemonic: None,
            derive_rotation_keys: None,
            address_rotation_interval: 86400,
            deployment: None,
            interval: 60,
            count: None,
            min_price_per_mcycle: parse_ether("0.001").unwrap(),
            max_price_per_mcycle: None,
            lock_collateral_raw: parse_ether("0.0").unwrap(),
            bidding_start_delay: None,
            ramp_up: 0,
            timeout: 1000,
            lock_timeout: 1000,
            seconds_per_mcycle: 60,
            ramp_up_seconds_per_mcycle: 60,
            exec_rate_khz: 5000,
            program: None,
            input: None,
            input_max_mcycles: None,
            warn_balance_below: None,
            error_balance_below: None,
            tx_timeout: 45,
            submit_retry_attempts: 3,
            submit_retry_delay_secs: 2,
            top_up_market_threshold: parse_ether("0.01").unwrap(),
            top_up_native_threshold: parse_ether("0.05").unwrap(),
            withdrawal_tx_timeout: 120,
            withdrawal_sweep_retries: 3,
            auto_deposit: None,
            submit_offchain: false,
            use_zeth: false,
            max_price_cap: None,
        }
    }

    /// Rejects derive_rotation_keys < 2.
    #[test]
    fn test_resolve_derive_rotation_keys_invalid() {
        let mut args = base_test_args();
        args.mnemonic = Some(BIP39_TEST_MNEMONIC.to_string());
        args.derive_rotation_keys = Some(1);
        let err = resolve_rotation_keys(&args).unwrap_err();
        assert!(err.to_string().contains(">= 2"));
    }

    /// Derives distinct funding and rotation keys from BIP-39 mnemonic.
    #[test]
    fn test_resolve_mnemonic_rotation_keys() {
        let mut args = base_test_args();
        args.mnemonic = Some(BIP39_TEST_MNEMONIC.to_string());
        args.derive_rotation_keys = Some(2);
        let result = resolve_rotation_keys(&args).unwrap();
        let (funding, rotation) = result.expect("should resolve to rotation from mnemonic");
        assert_eq!(rotation.len(), 2);
        assert_ne!(funding.address(), rotation[0].address());
        assert_ne!(rotation[0].address(), rotation[1].address());
    }

    /// Submits requests with a single key and verifies they appear on-chain.
    #[tokio::test]
    async fn test_main() {
        let anvil = Anvil::new().spawn();
        let ctx = create_test_ctx(&anvil).await.unwrap();

        let mut args = base_test_args();
        args.rpc_url = Some(anvil.endpoint_url());
        args.rpc_urls = Some(Vec::new());
        args.private_key = Some(ctx.customer_signer);
        args.deployment = Some(ctx.deployment.clone());
        args.interval = 1;
        args.count = Some(2);
        args.program = Some(LOOP_PATH.parse().unwrap());

        run(&args).await.unwrap();

        let filter = Filter::new()
            .event_signature(IBoundlessMarket::RequestSubmitted::SIGNATURE_HASH)
            .from_block(0)
            .address(ctx.deployment.boundless_market_address);
        let logs = ctx.customer_provider.get_logs(&filter).await.unwrap();
        let count = logs
            .iter()
            .filter_map(|log| log.log_decode::<IBoundlessMarket::RequestSubmitted>().ok())
            .count();
        assert_eq!(count, 2);
    }

    /// Full rotation lifecycle:
    ///   Phase 1 — submit 2 requests from whichever key desired_index selects.
    ///   Phase 2 — advance time so desired_index changes, submit 1 more request from
    ///             the new key, verify 3 total on-chain.
    ///
    /// Note: no indexer is available in this test, so the sweep of the old key is skipped
    /// gracefully (indexer unavailable warning). The test only verifies that submissions
    /// continue from the newly desired key.
    #[tokio::test]
    async fn test_rotation() {
        let anvil = Anvil::new().spawn();
        let ctx = create_test_ctx(&anvil).await.unwrap();

        let mut args = base_test_args();
        args.rpc_url = Some(anvil.endpoint_url());
        args.rpc_urls = Some(Vec::new());
        args.private_key = Some(ctx.customer_signer.clone());
        args.mnemonic = Some(BIP39_TEST_MNEMONIC.to_string());
        args.derive_rotation_keys = Some(2);
        args.address_rotation_interval = 86400; // long interval so we don't rotate mid-phase 1
        args.deployment = Some(ctx.deployment.clone());
        args.interval = 1;
        args.count = Some(2);
        args.program = Some(LOOP_PATH.parse().unwrap());

        // Phase 1: 2 requests from whichever key desired_index(now, 86400, 2) chose.
        run(&args).await.unwrap();

        let filter = Filter::new()
            .event_signature(IBoundlessMarket::RequestSubmitted::SIGNATURE_HASH)
            .from_block(0)
            .address(ctx.deployment.boundless_market_address);
        let count = ctx
            .customer_provider
            .get_logs(&filter)
            .await
            .unwrap()
            .iter()
            .filter_map(|l| l.log_decode::<IBoundlessMarket::RequestSubmitted>().ok())
            .count();
        assert_eq!(count, 2, "phase 1: expected 2 requests, got {count}");

        // Determine which key phase 1 used, then pick a target_ts that makes
        // desired_index(ts, 1, 2) = ts % 2 point to the *other* key.
        let block = ctx
            .customer_provider
            .get_block_by_number(BlockNumberOrTag::Latest)
            .await
            .unwrap()
            .unwrap();
        let phase1_ts = block.header.timestamp;
        let phase1_index = rotation::desired_index(phase1_ts, 86400, 2);
        // With interval=1: desired = ts % 2. We want ts % 2 != phase1_index.
        let target_ts =
            if phase1_ts % 2 == phase1_index as u64 { phase1_ts + 1 } else { phase1_ts };
        ctx.customer_provider.anvil_set_next_block_timestamp(target_ts).await.unwrap();
        ctx.customer_provider.anvil_mine(Some(1), None).await.unwrap();

        // Phase 2: rotation_interval=1 makes desired_index point to the other key → 1 request.
        args.address_rotation_interval = 1;
        args.count = Some(1);
        run(&args).await.unwrap();

        let count = ctx
            .customer_provider
            .get_logs(&filter)
            .await
            .unwrap()
            .iter()
            .filter_map(|l| l.log_decode::<IBoundlessMarket::RequestSubmitted>().ok())
            .count();
        assert_eq!(count, 3, "phase 2: expected 3 total requests after rotation, got {count}");
    }
}
