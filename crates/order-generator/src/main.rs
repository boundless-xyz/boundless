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
mod rotation;

use std::{path::Path, path::PathBuf, time::Duration};

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
    input::GuestEnv,
    request_builder::StandardRequestBuilder,
    storage::{HttpDownloader, StandardDownloader, StorageDownloader},
    StandardUploader, StorageUploaderConfig,
};
use clap::Parser;
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
    #[clap(long, env, requires = "derive_rotation_keys")]
    mnemonic: Option<String>,
    /// Derive N rotation keys from mnemonic using BIP-39/BIP-32 (m/44'/60'/0'/0/{0..N}).
    /// Index 0 = funding key; indices 1..N = rotation keys. Requires N >= 2 and --mnemonic.
    #[clap(long, env = "DERIVE_ROTATION_KEYS", requires = "mnemonic")]
    derive_rotation_keys: Option<usize>,
    /// Interval in seconds between address rotations (e.g., 86400 = daily).
    #[clap(long, default_value = "86400")]
    address_rotation_interval: u64,
    /// Path for persistent rotation state.
    #[clap(long, default_value = "./rotation-state.json")]
    rotation_state_file: PathBuf,
    /// Extra seconds after max request expiry before withdrawing from rotated address.
    #[clap(long, default_value = "60")]
    withdrawal_expiry_buffer: u64,
    /// Reset rotation state on startup (for testing).
    #[clap(long, default_value = "false")]
    rotation_state_reset: bool,
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
        Some((f, r)) if r.len() >= 2 => {
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

    let mut i = 0u64;
    loop {
        if let Some(count) = args.count {
            if i >= count {
                break;
            }
        }
        if let Err(e) = match args.use_zeth {
            true => {
                if let Some(input) = input.as_ref() {
                    handle_zeth_request(args, &client, &program, &program_url, input)
                        .await
                        .map(|_| ())
                } else {
                    let error_msg = "Zeth input is not provided".to_string();
                    tracing::error!("Request failed: {error_msg}");
                    return Err(anyhow::anyhow!(error_msg));
                }
            }
            false => handle_loop_request(args, &client, &program, &program_url).await.map(|_| ()),
        } {
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
    let state_path = &args.rotation_state_file;
    let mut state = rotation::RotationState::load(state_path);
    if args.rotation_state_reset {
        state = rotation::RotationState::default();
        state.save(state_path)?;
    }
    // Clamp last_used_index if state was from a run with more keys.
    if state.last_used_index >= n_keys {
        state.last_used_index = 0;
        state.save(state_path)?;
    }

    // Build a client for initial setup (program upload/download) using funding signer.
    let setup_client = build_client_for_signer(args, funding_signer).await?;
    let ipfs_gateway = args
        .storage_config
        .ipfs_gateway_url
        .clone()
        .unwrap_or(Url::parse("https://gateway.pinata.cloud").unwrap());
    let (program, program_url, input) =
        load_program_and_input(args, &setup_client, &ipfs_gateway).await?;

    let setup_provider = setup_client.provider();
    let now = get_block_timestamp(&setup_provider).await.unwrap_or_else(|e| {
        tracing::warn!("Failed to get block timestamp, using system time: {e:?}");
        rotation::now_secs()
    });
    try_pending_withdrawals(
        args,
        funding_signer,
        private_keys,
        &mut state,
        state_path,
        n_keys,
        now,
    )
    .await?;

    let mut last_index = state.last_used_index;
    let mut i = 0u64;
    let mut success_count = 0u64;
    let mut last_error: Option<anyhow::Error> = None;

    loop {
        if let Some(count) = args.count {
            if i >= count {
                break;
            }
        }

        let now = get_block_timestamp(&setup_provider).await.unwrap_or_else(|e| {
            tracing::warn!("Failed to get block timestamp, using system time: {e:?}");
            rotation::now_secs()
        });
        let idx = state.current_index(now, args.address_rotation_interval, n_keys);

        if idx != last_index {
            state.add_pending(last_index, state.max_expires_at(last_index));
            state.last_used_index = idx;
            state.last_rotation_timestamp = now;
            state.save(state_path)?;
            last_index = idx;
        }

        let signer = &private_keys[idx];
        let client = build_client_for_signer(args, signer).await?;
        top_up_from_funding_source(args, funding_signer, &client, signer.address()).await?;

        let result =
            submit_request_with_retry(args, &client, &program, &program_url, input.as_deref())
                .await;

        match result {
            Ok((_, expires_at)) => {
                success_count += 1;
                state.update_expires_at(idx, expires_at);
                state.save(state_path)?;
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

        let provider = client.provider();
        let now = get_block_timestamp(&provider).await.unwrap_or_else(|e| {
            tracing::warn!("Failed to get block timestamp, using system time: {e:?}");
            rotation::now_secs()
        });
        try_pending_withdrawals(
            args,
            funding_signer,
            private_keys,
            &mut state,
            state_path,
            n_keys,
            now,
        )
        .await?;

        i += 1;
        tokio::time::sleep(Duration::from_secs(args.interval)).await;
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
) -> Result<(U256, u64)> {
    let mut result = match args.use_zeth {
        true => {
            let input = zeth_input.ok_or_else(|| anyhow::anyhow!("Zeth input is not provided"))?;
            handle_zeth_request(args, client, program, program_url, input).await
        }
        false => handle_loop_request(args, client, program, program_url).await,
    };

    let max_retries = args.submit_retry_attempts;
    let retry_delay = Duration::from_secs(args.submit_retry_delay_secs);
    for attempt in 1..max_retries {
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

    client_builder
        .with_uploader_config(&args.storage_config)
        .await?
        .with_deployment(args.deployment.clone())
        .with_private_key(signer.clone())
        .with_balance_alerts(balance_alerts)
        .with_timeout(Some(Duration::from_secs(args.tx_timeout)))
        .with_funding_mode(FundingMode::BelowThreshold(args.top_up_market_threshold))
        .build()
        .await
}

/// Reserve left on funding/old address for gas (withdraw + transfer txs).
const GAS_RESERVE: &str = "0.001";

async fn top_up_from_funding_source(
    args: &MainArgs,
    funding_signer: &PrivateKeySigner,
    client: &OrderGeneratorClient,
    to_address: alloy::primitives::Address,
) -> Result<()> {
    let market_threshold = args.top_up_market_threshold;
    let native_threshold = args.top_up_native_threshold;
    let market = client.boundless_market.clone();
    let balance = market.balance_of(to_address).await.context("failed to get market balance")?;
    let native_balance =
        client.provider().get_balance(to_address).await.context("failed to get native balance")?;
    if balance >= market_threshold && native_balance >= native_threshold {
        return Ok(());
    }
    let funding_client = build_client_for_signer(args, funding_signer).await?;
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
    let needed = market_threshold.saturating_sub(balance).saturating_add(native_threshold);
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

async fn try_pending_withdrawals(
    args: &MainArgs,
    funding_signer: &PrivateKeySigner,
    private_keys: &[PrivateKeySigner],
    state: &mut rotation::RotationState,
    state_path: &Path,
    n_keys: usize,
    now_secs: u64,
) -> Result<()> {
    let buf = args.withdrawal_expiry_buffer;
    let pending: Vec<_> = state.pending_withdrawal.clone();
    for pw in pending {
        if state.can_withdraw(pw.index, now_secs, buf) {
            let idx = pw.index;
            if let Err(e) = withdraw_and_transfer(
                args,
                funding_signer,
                private_keys,
                state,
                state_path,
                pw,
                n_keys,
            )
            .await
            {
                tracing::warn!("Failed to withdraw and transfer from index {idx}: {e:?}");
            }
        }
    }
    Ok(())
}

async fn withdraw_and_transfer(
    args: &MainArgs,
    funding_signer: &PrivateKeySigner,
    private_keys: &[PrivateKeySigner],
    state: &mut rotation::RotationState,
    state_path: &Path,
    pw: rotation::PendingWithdrawal,
    n_keys: usize,
) -> Result<()> {
    if pw.index >= n_keys {
        tracing::warn!(
            "Skipping withdrawal for index {} (out of range, n_keys={})",
            pw.index,
            n_keys
        );
        state.remove_pending(pw.index);
        state.save(state_path)?;
        return Ok(());
    }
    let old_signer = &private_keys[pw.index];
    let funding_address = funding_signer.address();

    let old_client = build_client_for_signer(args, old_signer).await?;
    let market = old_client.boundless_market.clone();
    let balance = market
        .balance_of(old_signer.address())
        .await
        .context("failed to get market balance for withdrawal")?;
    let gas_reserve = parse_ether(GAS_RESERVE).unwrap();
    if balance <= gas_reserve {
        state.remove_pending(pw.index);
        state.save(state_path)?;
        return Ok(());
    }
    market.withdraw(balance).await.context("failed to withdraw from market")?;
    let transfer_amount = balance.saturating_sub(gas_reserve);
    old_client
        .transfer_value(funding_address, transfer_amount)
        .await
        .context("failed to transfer to funding address")?;
    state.remove_pending(pw.index);
    state.save(state_path)?;
    tracing::info!(
        "Withdrew {} ETH from index {} and transferred to funding address {}",
        format_units(balance, "ether")?,
        pw.index,
        funding_address
    );
    Ok(())
}

async fn handle_loop_request(
    args: &MainArgs,
    client: &OrderGeneratorClient,
    program: &[u8],
    program_url: &url::Url,
) -> Result<(U256, u64)> {
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
    Ok((request_id, expires_at))
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
) -> Result<(U256, u64)> {
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
    Ok((request_id, expires_at))
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
            rotation_state_file: PathBuf::from("./rotation-state.json"),
            withdrawal_expiry_buffer: 60,
            rotation_state_reset: false,
            deployment: None,
            interval: 60,
            count: None,
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
        args.rotation_state_file = std::env::temp_dir().join("og-main-test.json");
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

    /// Submits requests with key rotation and verifies all succeed.
    #[tokio::test]
    async fn test_rotation_flow() {
        let anvil = Anvil::new().spawn();
        let ctx = create_test_ctx(&anvil).await.unwrap();

        let state_file = std::env::temp_dir().join("og-rotation-flow-test.json");
        let _ = std::fs::remove_file(&state_file);

        let mut args = base_test_args();
        args.rpc_url = Some(anvil.endpoint_url());
        args.rpc_urls = Some(Vec::new());
        args.private_key = Some(ctx.customer_signer);
        args.mnemonic = Some(BIP39_TEST_MNEMONIC.to_string());
        args.derive_rotation_keys = Some(4);
        args.address_rotation_interval = 4;
        args.rotation_state_file = state_file;
        args.rotation_state_reset = true;
        args.deployment = Some(ctx.deployment.clone());
        args.interval = 4;
        args.count = Some(5);
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
        assert_eq!(count, 5, "expected 5 requests from rotation flow, got {count}");
    }

    /// Advances Anvil time past request expiry and verifies withdrawal from rotated addresses.
    #[tokio::test]
    async fn test_rotation_withdrawal() {
        let anvil = Anvil::new().spawn();
        let ctx = create_test_ctx(&anvil).await.unwrap();

        let state_file = std::env::temp_dir().join("og-rotation-withdrawal-test.json");
        let _ = std::fs::remove_file(&state_file);

        let mut args = base_test_args();
        args.rpc_url = Some(anvil.endpoint_url());
        args.rpc_urls = Some(Vec::new());
        args.private_key = Some(ctx.customer_signer);
        args.mnemonic = Some(BIP39_TEST_MNEMONIC.to_string());
        args.derive_rotation_keys = Some(2);
        args.address_rotation_interval = 6;
        args.rotation_state_file = state_file.clone();
        args.rotation_state_reset = true;
        args.deployment = Some(ctx.deployment.clone());
        args.interval = 4;
        args.count = Some(4);
        args.program = Some(LOOP_PATH.parse().unwrap());
        args.withdrawal_expiry_buffer = 60;

        run(&args).await.unwrap();

        let state = rotation::RotationState::load(&state_file);
        let max_expires_at = state
            .pending_withdrawal
            .iter()
            .map(|pw| pw.max_expires_at)
            .max()
            .expect("expected at least one pending withdrawal after rotation");

        let target_ts = max_expires_at + args.withdrawal_expiry_buffer + 1;
        ctx.customer_provider.anvil_set_next_block_timestamp(target_ts).await.unwrap();
        ctx.customer_provider.anvil_mine(Some(1), None).await.unwrap();

        args.count = Some(0);
        args.rotation_state_reset = false;
        run(&args).await.unwrap();

        let state_after = rotation::RotationState::load(&state_file);
        assert!(
            state_after.pending_withdrawal.is_empty(),
            "pending_withdrawal should be empty after withdrawals"
        );
    }
}
