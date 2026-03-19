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

use std::{collections::HashMap, path::PathBuf, sync::Arc};

use alloy::providers::{Provider, WalletProvider};
use anyhow::{Context, Result};
use boundless_market::contracts::boundless_market::BoundlessMarketService;
use broker::{
    build_chain_provider, config::ConfigWatcher, resolve_deployment, Args, Broker, ChainArgs,
    ChainPipeline,
};
use clap::Parser;
use tracing_subscriber::fmt::format::FmtSpan;
use url::Url;

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

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

    let base_config_watcher =
        ConfigWatcher::new(&args.config_file).await.context("Failed to load broker config")?;

    let discovered_chains = discover_chains(&args)?;

    let mut chain_pipelines = Vec::new();
    let mut _config_watchers: Vec<ConfigWatcher> = Vec::new();

    if discovered_chains.is_empty() {
        let private_key = args
            .private_key
            .clone()
            .or_else(|| std::env::var("PRIVATE_KEY").ok().and_then(|key| key.parse().ok()))
            .context(
                "Private key not provided. Set PROVER_PRIVATE_KEY or PRIVATE_KEY environment variable",
            )?;

        let rpc_urls =
            collect_rpc_urls(args.rpc_url.clone(), args.rpc_urls.clone(), args.experimental_rpc)?;

        let config = base_config_watcher.config.clone();
        let (provider, any_provider, gas_priority_mode) =
            build_chain_provider(&rpc_urls, &private_key, &args, &config)?;
        let provider = Arc::new(provider);

        let chain_id = provider.get_chain_id().await.context("Failed to get chain ID")?;
        let deployment = resolve_deployment(args.deployment.as_ref(), chain_id)?;
        tracing::info!(chain_id, %deployment, "Using deployment configuration");

        if !args.listen_only {
            if let Some(deposit_amount) = args.deposit_amount.as_ref() {
                let market = BoundlessMarketService::new_for_broker(
                    deployment.boundless_market_address,
                    provider.clone(),
                    provider.default_signer_address(),
                );
                tracing::info!(
                    chain_id,
                    "Pre-depositing {deposit_amount} stake tokens into the market contract"
                );
                market
                    .deposit_collateral_with_permit(*deposit_amount, &private_key)
                    .await
                    .context("Failed to deposit to market")?;
            }
        }

        chain_pipelines.push(ChainPipeline {
            provider,
            any_provider,
            config,
            gas_priority_mode,
            private_key,
            chain_id,
            deployment,
        });
    } else {
        for chain_args in &discovered_chains {
            tracing::info!(
                chain_id = chain_args.chain_id,
                rpc_urls = ?chain_args.rpc_urls,
                "Starting pipeline for chain"
            );

            let chain_config_watcher = ConfigWatcher::new_with_override(
                &args.config_file,
                chain_args.config_override_path.as_deref(),
            )
            .await
            .with_context(|| format!("Failed to load config for chain {}", chain_args.chain_id))?;

            let config = chain_config_watcher.config.clone();
            let (provider, any_provider, gas_priority_mode) = build_chain_provider(
                &chain_args.rpc_urls,
                &chain_args.private_key,
                &args,
                &config,
            )?;
            let provider = Arc::new(provider);

            let chain_id = provider.get_chain_id().await.context("Failed to get chain ID")?;
            let deployment = chain_args
                .deployment
                .clone()
                .map(Ok)
                .unwrap_or_else(|| resolve_deployment(args.deployment.as_ref(), chain_id))?;
            tracing::info!(chain_id, %deployment, "Using deployment configuration");

            if !args.listen_only {
                if let Some(deposit_amount) = args.deposit_amount.as_ref() {
                    let market = BoundlessMarketService::new_for_broker(
                        deployment.boundless_market_address,
                        provider.clone(),
                        provider.default_signer_address(),
                    );
                    tracing::info!(
                        chain_id,
                        "Pre-depositing {deposit_amount} stake tokens into the market contract"
                    );
                    market
                        .deposit_collateral_with_permit(*deposit_amount, &chain_args.private_key)
                        .await
                        .context("Failed to deposit to market")?;
                }
            }

            chain_pipelines.push(ChainPipeline {
                provider,
                any_provider,
                config,
                gas_priority_mode,
                private_key: chain_args.private_key.clone(),
                chain_id,
                deployment,
            });

            _config_watchers.push(chain_config_watcher);
        }
    }

    let broker = Broker::new(args, base_config_watcher).await?;

    broker.start_service(chain_pipelines).await.context("Broker service failed")?;

    Ok(())
}

fn collect_rpc_urls(
    rpc_url: Option<String>,
    rpc_urls: Vec<String>,
    experimental_rpc: bool,
) -> Result<Vec<Url>> {
    let mut all_rpc_urls: Vec<Url> = Vec::new();

    if let Some(url_str) = rpc_url {
        if !url_str.is_empty() {
            let url =
                Url::parse(&url_str).context("Invalid PROVER_RPC_URL environment variable")?;
            if !all_rpc_urls.contains(&url) {
                all_rpc_urls.push(url);
            }
        }
    }

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

    if all_rpc_urls.is_empty() {
        anyhow::bail!(
            "No RPC URLs provided. Please set at least one using PROVER_RPC_URL or PROVER_RPC_URLS environment variables"
        );
    }

    Ok(all_rpc_urls)
}

fn parse_chain_id_values<T: std::str::FromStr>(
    entries: &[String],
    arg_name: &str,
) -> Result<HashMap<u64, T>>
where
    T::Err: std::fmt::Display,
{
    let mut map = HashMap::new();
    for entry in entries {
        let (chain_id_str, value_str) = entry.split_once(':').with_context(|| {
            format!("Invalid {arg_name} format '{entry}', expected {{chain_id}}:{{value}}")
        })?;
        let chain_id: u64 = chain_id_str
            .parse()
            .with_context(|| format!("Invalid chain_id '{chain_id_str}' in {arg_name}"))?;
        let value: T = value_str.parse().map_err(|e| {
            anyhow::anyhow!("Invalid value for chain {chain_id} in {arg_name}: {e}")
        })?;
        map.insert(chain_id, value);
    }
    Ok(map)
}

fn build_chain_deployments(args: &Args) -> Result<HashMap<u64, boundless_market::Deployment>> {
    use alloy::primitives::Address;
    use std::borrow::Cow;

    let market_addrs: HashMap<u64, Address> =
        parse_chain_id_values(&args.chain_market_address, "--chain-market-address")?;
    let set_verifier_addrs: HashMap<u64, Address> =
        parse_chain_id_values(&args.chain_set_verifier_address, "--chain-set-verifier-address")?;
    let verifier_router_addrs: HashMap<u64, Address> = parse_chain_id_values(
        &args.chain_verifier_router_address,
        "--chain-verifier-router-address",
    )?;
    let collateral_token_addrs: HashMap<u64, Address> = parse_chain_id_values(
        &args.chain_collateral_token_address,
        "--chain-collateral-token-address",
    )?;
    let order_stream_urls: HashMap<u64, String> =
        parse_chain_id_values(&args.chain_order_stream_url, "--chain-order-stream-url")?;

    let mut deployments = HashMap::new();
    // Collect all chain IDs mentioned in any deployment arg
    let mut chain_ids: std::collections::BTreeSet<u64> = std::collections::BTreeSet::new();
    chain_ids.extend(market_addrs.keys());
    chain_ids.extend(set_verifier_addrs.keys());

    for chain_id in chain_ids {
        let market_address = market_addrs.get(&chain_id).with_context(|| {
            format!("--chain-market-address required for chain {chain_id} (--chain-set-verifier-address was provided)")
        })?;
        let set_verifier_address = set_verifier_addrs.get(&chain_id).with_context(|| {
            format!("--chain-set-verifier-address required for chain {chain_id} (--chain-market-address was provided)")
        })?;

        let mut builder = boundless_market::Deployment::builder();
        builder
            .market_chain_id(chain_id)
            .boundless_market_address(*market_address)
            .set_verifier_address(*set_verifier_address);

        if let Some(addr) = verifier_router_addrs.get(&chain_id) {
            builder.verifier_router_address(*addr);
        }
        if let Some(addr) = collateral_token_addrs.get(&chain_id) {
            builder.collateral_token_address(*addr);
        }
        if let Some(url) = order_stream_urls.get(&chain_id) {
            builder.order_stream_url(Cow::Owned(url.clone()));
        }

        deployments.insert(
            chain_id,
            builder
                .build()
                .with_context(|| format!("Failed to build deployment for chain {chain_id}"))?,
        );
    }

    Ok(deployments)
}

fn discover_chains(args: &Args) -> Result<Vec<ChainArgs>> {
    use std::collections::BTreeMap;

    // Parse --chain-config-file overrides
    let mut config_overrides: HashMap<u64, PathBuf> = HashMap::new();
    for entry in &args.chain_config_file {
        let (chain_id_str, path_str) = entry.split_once(':').with_context(|| {
            format!("Invalid --chain-config-file format '{entry}', expected {{chain_id}}:{{path}}")
        })?;
        let chain_id: u64 = chain_id_str
            .parse()
            .with_context(|| format!("Invalid chain_id '{chain_id_str}' in --chain-config-file"))?;
        config_overrides.insert(chain_id, PathBuf::from(path_str));
    }

    // Parse --chain-private-keys
    let mut explicit_keys: HashMap<u64, alloy::signers::local::PrivateKeySigner> = HashMap::new();
    for entry in &args.chain_private_keys {
        let (chain_id_str, key_str) = entry.split_once(':').with_context(|| {
            format!("Invalid --chain-private-keys format '{entry}', expected {{chain_id}}:{{key}}")
        })?;
        let chain_id: u64 = chain_id_str.parse().with_context(|| {
            format!("Invalid chain_id '{chain_id_str}' in --chain-private-keys")
        })?;
        let key: alloy::signers::local::PrivateKeySigner = key_str.parse().with_context(|| {
            format!("Invalid private key for chain {chain_id} in --chain-private-keys")
        })?;
        explicit_keys.insert(chain_id, key);
    }

    // Collect chains: --chain-rpc-urls first, then env vars for chains not already specified
    let mut chain_urls: BTreeMap<u64, Vec<Url>> = BTreeMap::new();

    for entry in &args.chain_rpc_urls {
        let (chain_id_str, urls_str) = entry.split_once(':').with_context(|| {
            format!(
                "Invalid --chain-rpc-urls format '{entry}', expected {{chain_id}}:{{url1}},{{url2}},..."
            )
        })?;
        let chain_id: u64 = chain_id_str
            .parse()
            .with_context(|| format!("Invalid chain_id '{chain_id_str}' in --chain-rpc-urls"))?;
        for url_str in urls_str.split(',') {
            let url_str = url_str.trim();
            if url_str.is_empty() {
                continue;
            }
            let url = Url::parse(url_str)
                .with_context(|| format!("Invalid URL in --chain-rpc-urls for chain {chain_id}"))?;
            let urls = chain_urls.entry(chain_id).or_default();
            if !urls.contains(&url) {
                urls.push(url);
            }
        }
    }

    // Env vars for chains not already specified via CLI
    for (key, _) in std::env::vars() {
        let Some(suffix) = key.strip_prefix("PROVER_RPC_URL_") else {
            continue;
        };
        if key.starts_with("PROVER_RPC_URLS_") {
            continue;
        }
        let chain_id: u64 = match suffix.parse() {
            Ok(id) => id,
            Err(_) => continue,
        };
        if chain_urls.contains_key(&chain_id) {
            continue;
        }

        let primary_url = std::env::var(format!("PROVER_RPC_URL_{chain_id}"))
            .with_context(|| format!("PROVER_RPC_URL_{chain_id} is set but empty"))?;
        let mut urls = vec![Url::parse(&primary_url)
            .with_context(|| format!("Invalid PROVER_RPC_URL_{chain_id}"))?];

        if let Ok(extra_urls) = std::env::var(format!("PROVER_RPC_URLS_{chain_id}")) {
            for url_str in extra_urls.split(',') {
                let url_str = url_str.trim();
                if url_str.is_empty() {
                    continue;
                }
                let url = Url::parse(url_str)
                    .with_context(|| format!("Invalid URL in PROVER_RPC_URLS_{chain_id}"))?;
                if !urls.contains(&url) {
                    urls.push(url);
                }
            }
        }

        chain_urls.insert(chain_id, urls);
    }

    if chain_urls.is_empty() {
        return Ok(Vec::new());
    }

    let mut chain_deployments = build_chain_deployments(args)?;

    let global_private_key = args
        .private_key
        .clone()
        .or_else(|| std::env::var("PRIVATE_KEY").ok().and_then(|key| key.parse().ok()));

    let mut chains = Vec::with_capacity(chain_urls.len());
    for (chain_id, rpc_urls) in chain_urls {
        // Resolution order: --chain-private-keys > PROVER_PRIVATE_KEY_{id} > global key
        let private_key = explicit_keys
            .remove(&chain_id)
            .or_else(|| {
                std::env::var(format!("PROVER_PRIVATE_KEY_{chain_id}"))
                    .ok()
                    .and_then(|key| key.parse().ok())
            })
            .or_else(|| global_private_key.clone())
            .with_context(|| {
                format!(
                    "No private key for chain {chain_id}. \
                     Set --chain-private-keys {chain_id}:{{key}} or PROVER_PRIVATE_KEY_{chain_id} or PROVER_PRIVATE_KEY"
                )
            })?;

        let config_override_path = config_overrides
            .remove(&chain_id)
            .or_else(|| ConfigWatcher::override_path_for_chain(&args.config_file, chain_id));

        let deployment = chain_deployments.remove(&chain_id);

        chains.push(ChainArgs {
            chain_id,
            rpc_urls,
            private_key,
            config_override_path,
            deployment,
        });
    }

    for (chain_id, path) in &config_overrides {
        tracing::warn!(
            "--chain-config-file specified for chain {chain_id} ({}) but no RPC URLs found for chain, ignoring",
            path.display()
        );
    }

    Ok(chains)
}

#[cfg(test)]
mod tests {
    use super::*;
    use broker::Args;
    use std::path::PathBuf;

    // Anvil default test accounts -- only used as valid distinct private key values in assertions
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
            chain_rpc_urls: vec![],
            chain_private_keys: vec![],
            chain_config_file: vec![],
            chain_market_address: vec![],
            chain_set_verifier_address: vec![],
            chain_verifier_router_address: vec![],
            chain_collateral_token_address: vec![],
            chain_order_stream_url: vec![],
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
        assert_eq!(chains[0].chain_id, 1);
        assert_eq!(chains[0].private_key, chain_key.parse().unwrap());
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
    fn discover_chains_explicit_chain_config_file() {
        let _lock = ENV_LOCK.lock().unwrap();
        clear_chain_env_vars();

        std::env::set_var("PROVER_RPC_URL_8453", "http://base.example.com");

        let mut args = default_args();
        args.private_key = Some(TEST_PRIVATE_KEY_0.parse().unwrap());
        args.chain_config_file = vec!["8453:/custom/path/broker.base.toml".into()];

        let chains = discover_chains(&args).unwrap();
        assert_eq!(chains.len(), 1);
        assert_eq!(
            chains[0].config_override_path,
            Some(PathBuf::from("/custom/path/broker.base.toml"))
        );

        clear_chain_env_vars();
    }
}
