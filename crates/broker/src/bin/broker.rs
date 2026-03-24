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

use alloy::transports::http::reqwest;
use alloy::{
    network::AnyNetwork,
    primitives::utils::parse_ether,
    providers::{
        fillers::ChainIdFiller, network::EthereumWallet, DynProvider, ProviderBuilder,
        WalletProvider,
    },
    rpc::client::RpcClient,
    transports::{http::Http, layers::RetryBackoffLayer},
};
use anyhow::{Context, Result};
use boundless_market::{
    balance_alerts_layer::{BalanceAlertConfig, BalanceAlertLayer},
    contracts::boundless_market::BoundlessMarketService,
    dynamic_gas_filler::DynamicGasFiller,
    nonce_layer::NonceProvider,
};
use broker::{
    config::ConfigWatcher, rpcmetrics::RpcMetricsLayer,
    sequential_fallback::SequentialFallbackLayer, Args, Broker, ChainArgs, CustomRetryPolicy,
};
use clap::Parser;
use std::{collections::HashMap, path::PathBuf, time::Duration};
use tower::{Layer, ServiceBuilder};
use tracing_subscriber::fmt::format::FmtSpan;
use url::Url;

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    let all_rpc_urls =
        collect_rpc_urls(args.rpc_url.clone(), args.rpc_urls.clone(), args.experimental_rpc)?;

    if args.log_json {
        tracing_subscriber::fmt()
            .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
            .with_span_events(FmtSpan::CLOSE)
            .json()
            .with_ansi(false)
            .init();
    } else {
        tracing_subscriber::fmt()
            .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
            .with_span_events(FmtSpan::CLOSE)
            .init();
    }

    // Discover per-chain configurations from PROVER_RPC_URL_{chain_id} env vars
    let chains = discover_chains(&args)?;
    if !chains.is_empty() {
        for chain in &chains {
            tracing::info!(
                chain_id = chain.chain_id,
                rpc_urls = ?chain.rpc_urls,
                config_override = chain.config_override_path.as_ref().map(|p| p.display().to_string()).unwrap_or_default(),
                "Discovered chain"
            );
        }
        anyhow::bail!(
            "Multi-chain mode detected ({} chains: {:?}). Multi-chain broker not yet implemented.",
            chains.len(),
            chains.iter().map(|c| c.chain_id).collect::<Vec<_>>()
        );
    }

    // Create config watcher early so we can use it for the gas filler
    let config_watcher =
        ConfigWatcher::new(&args.config_file).await.context("Failed to load broker config")?;

    // Handle private key fallback: PROVER_PRIVATE_KEY -> PRIVATE_KEY
    let private_key = args
        .private_key
        .clone()
        .or_else(|| std::env::var("PRIVATE_KEY").ok().and_then(|key| key.parse().ok()))
        .context(
            "Private key not provided. Set PROVER_PRIVATE_KEY or PRIVATE_KEY environment variable",
        )?;

    let wallet = EthereumWallet::from(private_key.clone());

    let retry_layer = RetryBackoffLayer::new_with_policy(
        args.rpc_retry_max,
        args.rpc_retry_backoff,
        args.rpc_retry_cu,
        CustomRetryPolicy,
    );

    let mut http_client_builder = reqwest::Client::builder();
    if args.rpc_request_timeout > 0 {
        http_client_builder =
            http_client_builder.timeout(Duration::from_secs(args.rpc_request_timeout));
    }
    let http_client = http_client_builder.build().expect("Failed to build HTTP client");

    // Build RPC client with fallback support if multiple URLs are provided
    let client = if all_rpc_urls.len() > 1 {
        // Multiple URLs - sequential fallback: always try primary first, only fall back on failure.
        // On a single RPC failure the sequential fallback immediately tries the next URL.
        // The outer RetryBackoffLayer only kicks in after all URLs have been exhausted.
        let transports: Vec<_> = all_rpc_urls
            .iter()
            .map(|url| {
                RpcMetricsLayer::new().layer(Http::with_client(http_client.clone(), url.clone()))
            })
            .collect();

        tracing::info!(
            "Configuring broker with sequential fallback RPC support: {} URLs: {:?}",
            all_rpc_urls.len(),
            all_rpc_urls
        );

        let transport = ServiceBuilder::new()
            .layer(retry_layer)
            .layer(SequentialFallbackLayer)
            .service(transports);

        RpcClient::builder().transport(transport, false)
    } else {
        // Single URL - use regular provider
        let single_url = &all_rpc_urls[0];
        tracing::info!("Configuring broker with single RPC URL: {}", single_url);
        let transport = ServiceBuilder::new().layer(retry_layer).service(
            RpcMetricsLayer::new().layer(Http::with_client(http_client, single_url.clone())),
        );
        RpcClient::builder().transport(transport, false)
    };

    // Read config for balance alerts (scope the guard so we can move config_watcher later)
    let balance_alerts_config = {
        let config = config_watcher.config.lock_all().context("Failed to read config")?;
        BalanceAlertConfig {
            watch_address: wallet.default_signer().address(),
            warn_threshold: config
                .market
                .balance_warn_threshold
                .as_ref()
                .map(|s| parse_ether(s))
                .transpose()?,
            error_threshold: config
                .market
                .balance_error_threshold
                .as_ref()
                .map(|s| parse_ether(s))
                .transpose()?,
        }
    };
    let balance_alerts_layer = BalanceAlertLayer::new(balance_alerts_config);

    let priority_mode = {
        let config = config_watcher.config.lock_all().context("Failed to read config")?;
        config.market.gas_priority_mode.clone()
    };
    let dynamic_gas_filler = DynamicGasFiller::new(
        20, // 20% increase of gas limit
        priority_mode,
        wallet.default_signer().address(),
    );

    // Clone the priority_mode Arc so we can pass it to the broker for runtime updates
    let gas_priority_mode = dynamic_gas_filler.priority_mode.clone();

    let base_provider = ProviderBuilder::new()
        .disable_recommended_fillers()
        .filler(ChainIdFiller::default())
        .filler(dynamic_gas_filler)
        .layer(balance_alerts_layer)
        .connect_client(client.clone());

    let provider = NonceProvider::new(base_provider, wallet.clone());

    // Build a separate AnyNetwork provider for get_block_receipts, reusing the same transport.
    // Needed on OP Stack chains where deposit receipts don't fit the standard Ethereum type.
    // RpcClient is Clone (Arc-backed), so both providers share the same underlying connection.
    let any_provider = DynProvider::new(
        ProviderBuilder::new()
            .network::<AnyNetwork>()
            .filler(ChainIdFiller::default())
            .connect_client(client),
    );

    let broker = Broker::new(
        args.clone(),
        provider.clone(),
        any_provider,
        config_watcher,
        gas_priority_mode,
    )
    .await?;

    // TODO: Move this code somewhere else / monitor our balanceOf and top it up as needed
    if !args.listen_only {
        if let Some(deposit_amount) = args.deposit_amount.as_ref() {
            let boundless_market = BoundlessMarketService::new_for_broker(
                broker.deployment().boundless_market_address,
                provider.clone(),
                provider.default_signer_address(),
            );

            tracing::info!("pre-depositing {deposit_amount} stake tokens into the market contract");
            boundless_market
                .deposit_collateral_with_permit(*deposit_amount, &private_key)
                .await
                .context("Failed to deposit to market")?;
        }
    }

    // Await broker shutdown before returning from main
    broker.start_service().await.context("Broker service failed")?;

    Ok(())
}

/// Collect and parse all RPC URLs from args and environment variables
/// Returns a deduplicated list of valid RPC URLs
fn collect_rpc_urls(
    rpc_url: Option<String>,
    rpc_urls: Vec<String>,
    experimental_rpc: bool,
) -> Result<Vec<Url>> {
    // Use Vec to preserve insertion order: PROVER_RPC_URL is always inserted first,
    // ensuring it lands at index 0 for the sequential fallback transport.
    let mut all_rpc_urls: Vec<Url> = Vec::new();

    // Parse PROVER_RPC_URL (ignore if empty)
    if let Some(url_str) = rpc_url {
        if !url_str.is_empty() {
            let url =
                Url::parse(&url_str).context("Invalid PROVER_RPC_URL environment variable")?;
            if !all_rpc_urls.contains(&url) {
                all_rpc_urls.push(url);
            }
        }
    }

    // Parse PROVER_RPC_URLS (ignore empty strings, split by comma)
    for url_str in rpc_urls {
        let url_str = url_str.trim();
        if !url_str.is_empty() {
            let url =
                Url::parse(url_str).context("Invalid PROVER_RPC_URLS environment variable")?;
            if !all_rpc_urls.contains(&url) {
                all_rpc_urls.push(url);
            }
        }
    }

    // Backward compatibility: check RPC_URL if no URLs collected yet
    if all_rpc_urls.is_empty() {
        if let Ok(legacy_rpc_url) = std::env::var("RPC_URL") {
            if !legacy_rpc_url.is_empty() {
                let url =
                    Url::parse(&legacy_rpc_url).context("Invalid RPC_URL environment variable")?;
                all_rpc_urls.push(url);
                tracing::info!("Using RPC_URL environment variable (PROVER_RPC_URL not set)");
            }
        }
    }

    if all_rpc_urls.is_empty() && experimental_rpc {
        all_rpc_urls.push(
            Url::parse("https://base.gateway.tenderly.co")
                .expect("hardcoded Tenderly URL is valid"),
        );
    }

    // Error early if no RPC URLs provided
    if all_rpc_urls.is_empty() {
        anyhow::bail!(
            "No RPC URLs provided. Please set at least one using PROVER_RPC_URL or PROVER_RPC_URLS environment variables"
        );
    }

    Ok(all_rpc_urls)
}

/// Discover enabled chains by scanning environment variables for `PROVER_RPC_URL_{chain_id}`.
///
/// Returns an empty vec if no per-chain env vars are found (single-chain mode).
fn discover_chains(args: &Args) -> Result<Vec<ChainArgs>> {
    // Parse explicit --chain-config args into a map: chain_id -> path
    let mut explicit_overrides: HashMap<u64, PathBuf> = HashMap::new();
    for entry in &args.chain_config {
        let (chain_id_str, path_str) = entry.split_once(':').with_context(|| {
            format!("Invalid --chain-config format '{entry}', expected {{chain_id}}:{{path}}")
        })?;
        let chain_id: u64 = chain_id_str
            .parse()
            .with_context(|| format!("Invalid chain_id '{chain_id_str}' in --chain-config"))?;
        explicit_overrides.insert(chain_id, PathBuf::from(path_str));
    }

    // Scan env for PROVER_RPC_URL_{chain_id}
    let mut chain_ids: Vec<u64> = Vec::new();
    for (key, _) in std::env::vars() {
        let Some(suffix) = key.strip_prefix("PROVER_RPC_URL_") else {
            continue;
        };
        // Skip PROVER_RPC_URLS_{chain_id} (note the trailing S)
        if key.starts_with("PROVER_RPC_URLS_") {
            continue;
        }
        let chain_id: u64 = match suffix.parse() {
            Ok(id) => id,
            Err(_) => continue, // not a valid chain_id suffix, skip
        };
        if !chain_ids.contains(&chain_id) {
            chain_ids.push(chain_id);
        }
    }

    chain_ids.sort();

    if chain_ids.is_empty() {
        return Ok(Vec::new());
    }

    // Resolve the global private key for fallback
    let global_private_key = args
        .private_key
        .clone()
        .or_else(|| std::env::var("PRIVATE_KEY").ok().and_then(|key| key.parse().ok()));

    let mut chains = Vec::with_capacity(chain_ids.len());
    for chain_id in chain_ids {
        // Collect RPC URLs for this chain
        let primary_url = std::env::var(format!("PROVER_RPC_URL_{chain_id}"))
            .with_context(|| format!("PROVER_RPC_URL_{chain_id} is set but empty"))?;

        let mut rpc_urls = vec![Url::parse(&primary_url)
            .with_context(|| format!("Invalid PROVER_RPC_URL_{chain_id}"))?];

        if let Ok(extra_urls) = std::env::var(format!("PROVER_RPC_URLS_{chain_id}")) {
            for url_str in extra_urls.split(',') {
                let url_str = url_str.trim();
                if url_str.is_empty() {
                    continue;
                }
                let url = Url::parse(url_str)
                    .with_context(|| format!("Invalid URL in PROVER_RPC_URLS_{chain_id}"))?;
                if !rpc_urls.contains(&url) {
                    rpc_urls.push(url);
                }
            }
        }

        // Resolve private key: per-chain -> global PROVER_PRIVATE_KEY -> legacy PRIVATE_KEY
        let private_key = std::env::var(format!("PROVER_PRIVATE_KEY_{chain_id}"))
            .ok()
            .and_then(|key| key.parse().ok())
            .or_else(|| global_private_key.clone())
            .with_context(|| {
                format!(
                    "No private key for chain {chain_id}. \
                     Set PROVER_PRIVATE_KEY_{chain_id} or PROVER_PRIVATE_KEY"
                )
            })?;

        // Resolve config override: explicit --chain-config > auto-discover broker.{chain_id}.toml
        let config_override_path = explicit_overrides
            .remove(&chain_id)
            .or_else(|| ConfigWatcher::override_path_for_chain(&args.config_file, chain_id));

        chains.push(ChainArgs { chain_id, rpc_urls, private_key, config_override_path });
    }

    // Warn about --chain-config entries for chains that aren't enabled
    for (chain_id, path) in &explicit_overrides {
        tracing::warn!(
            "--chain-config specified for chain {chain_id} ({}) but no PROVER_RPC_URL_{chain_id} found, ignoring",
            path.display()
        );
    }

    Ok(chains)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Anvil default test accounts — only used as valid distinct private key values in assertions
    const TEST_PRIVATE_KEY_0: &str =
        "0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80";
    const TEST_PRIVATE_KEY_1: &str =
        "0x59c6995e998f97a5a0044966f0945389dc9e86dae88c7a8412f4603b6b78690d";

    #[test]
    fn primary_url_is_first() {
        let result = collect_rpc_urls(
            Some("http://primary.example.com".to_string()),
            vec![
                "http://secondary.example.com".to_string(),
                "http://tertiary.example.com".to_string(),
            ],
            false,
        )
        .unwrap();
        assert_eq!(result[0].as_str(), "http://primary.example.com/");
    }

    #[test]
    fn deduplicates_urls() {
        let result = collect_rpc_urls(
            Some("http://node.example.com".to_string()),
            vec!["http://node.example.com".to_string()],
            false,
        )
        .unwrap();
        assert_eq!(result.len(), 1);
    }

    #[test]
    fn errors_on_empty() {
        let result = collect_rpc_urls(None, vec![], false);
        assert!(result.is_err());
    }

    // Env-mutating tests must run serially to avoid interference.
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn default_args() -> Args {
        Args {
            db_url: "sqlite::memory:".into(),
            rpc_url: None,
            rpc_urls: vec![],
            private_key: None,
            deployment: None,
            bento_api_url: None,
            bonsai_api_url: None,
            bonsai_api_key: None,
            config_file: PathBuf::from("broker.toml"),
            deposit_amount: None,
            rpc_retry_max: 10,
            rpc_retry_backoff: 1000,
            rpc_retry_cu: 100,
            rpc_request_timeout: 15,
            log_json: false,
            listen_only: false,
            experimental_rpc: false,
            chain_config: vec![],
        }
    }

    fn clear_chain_env_vars() {
        let keys_to_remove: Vec<String> = std::env::vars()
            .filter_map(|(key, _)| {
                if key.starts_with("PROVER_RPC_URL_")
                    || key.starts_with("PROVER_RPC_URLS_")
                    || key.starts_with("PROVER_PRIVATE_KEY_")
                {
                    Some(key)
                } else {
                    None
                }
            })
            .collect();
        for key in keys_to_remove {
            std::env::remove_var(&key);
        }
    }

    #[test]
    fn discover_chains_empty_when_no_per_chain_vars() {
        let _lock = ENV_LOCK.lock().unwrap();
        clear_chain_env_vars();

        let args = default_args();
        let chains = discover_chains(&args).unwrap();
        assert!(chains.is_empty());
    }

    #[test]
    fn discover_chains_finds_chains_from_env() {
        let _lock = ENV_LOCK.lock().unwrap();
        clear_chain_env_vars();

        std::env::set_var("PROVER_RPC_URL_8453", "http://base.example.com");
        std::env::set_var("PROVER_RPC_URL_1", "http://eth.example.com");
        std::env::set_var("PROVER_PRIVATE_KEY", TEST_PRIVATE_KEY_0);

        let mut args = default_args();
        args.private_key = Some(TEST_PRIVATE_KEY_0.parse().unwrap());

        let chains = discover_chains(&args).unwrap();
        assert_eq!(chains.len(), 2);
        assert_eq!(chains[0].chain_id, 1);
        assert_eq!(chains[0].rpc_urls[0].as_str(), "http://eth.example.com/");
        assert_eq!(chains[1].chain_id, 8453);
        assert_eq!(chains[1].rpc_urls[0].as_str(), "http://base.example.com/");

        clear_chain_env_vars();
    }

    #[test]
    fn discover_chains_per_chain_private_key() {
        let _lock = ENV_LOCK.lock().unwrap();
        clear_chain_env_vars();

        let global_key = TEST_PRIVATE_KEY_0;
        let chain_key = TEST_PRIVATE_KEY_1;

        std::env::set_var("PROVER_RPC_URL_8453", "http://base.example.com");
        std::env::set_var("PROVER_RPC_URL_1", "http://eth.example.com");
        std::env::set_var("PROVER_PRIVATE_KEY_1", chain_key);

        let mut args = default_args();
        args.private_key = Some(global_key.parse().unwrap());

        let chains = discover_chains(&args).unwrap();
        // Chain 1 should use its own key
        assert_eq!(chains[0].chain_id, 1);
        assert_eq!(chains[0].private_key, chain_key.parse().unwrap());
        // Chain 8453 should fall back to global
        assert_eq!(chains[1].chain_id, 8453);
        assert_eq!(chains[1].private_key, global_key.parse().unwrap());

        clear_chain_env_vars();
        std::env::remove_var("PROVER_PRIVATE_KEY_1");
    }

    #[test]
    fn discover_chains_failover_urls() {
        let _lock = ENV_LOCK.lock().unwrap();
        clear_chain_env_vars();

        std::env::set_var("PROVER_RPC_URL_8453", "http://primary.example.com");
        std::env::set_var(
            "PROVER_RPC_URLS_8453",
            "http://backup1.example.com,http://backup2.example.com",
        );

        let mut args = default_args();
        args.private_key = Some(TEST_PRIVATE_KEY_0.parse().unwrap());

        let chains = discover_chains(&args).unwrap();
        assert_eq!(chains.len(), 1);
        assert_eq!(chains[0].rpc_urls.len(), 3);
        assert_eq!(chains[0].rpc_urls[0].as_str(), "http://primary.example.com/");
        assert_eq!(chains[0].rpc_urls[1].as_str(), "http://backup1.example.com/");
        assert_eq!(chains[0].rpc_urls[2].as_str(), "http://backup2.example.com/");

        clear_chain_env_vars();
    }

    #[test]
    fn discover_chains_explicit_chain_config() {
        let _lock = ENV_LOCK.lock().unwrap();
        clear_chain_env_vars();

        std::env::set_var("PROVER_RPC_URL_8453", "http://base.example.com");

        let mut args = default_args();
        args.private_key = Some(TEST_PRIVATE_KEY_0.parse().unwrap());
        args.chain_config = vec!["8453:/custom/path/broker.base.toml".into()];

        let chains = discover_chains(&args).unwrap();
        assert_eq!(chains.len(), 1);
        assert_eq!(
            chains[0].config_override_path,
            Some(PathBuf::from("/custom/path/broker.base.toml"))
        );

        clear_chain_env_vars();
    }
}
