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

use std::{
    borrow::Cow,
    collections::{BTreeMap, BTreeSet, HashMap},
    path::PathBuf,
    sync::Arc,
};

use alloy::{
    primitives::Address,
    providers::{Provider, WalletProvider},
    signers::local::PrivateKeySigner,
};
use anyhow::{Context, Result};
use boundless_market::contracts::boundless_market::BoundlessMarketService;
use broker::{
    broker_sqlite_url_for_chain, build_chain_provider, config::ConfigWatcher, resolve_deployment,
    Broker, ChainArgs, ChainPipeline, CoreArgs, SqliteDb,
};
use clap::{Arg, ArgAction, CommandFactory, FromArgMatches};
use tracing_subscriber::fmt::format::FmtSpan;
use url::Url;

struct ChainArgDef {
    name: &'static str,
    help: &'static str,
    append: bool,
}

/// Merge a primary URL with an iterator of additional URLs into a deduplicated `Vec<Url>`.
///
/// Empty-after-trim entries are skipped. `err_label` is included in the parse-error context
/// (e.g. `"PROVER_RPC_URL(S)_8453"`).
fn merge_rpc_urls<'a>(
    primary: Option<&'a str>,
    list: impl IntoIterator<Item = &'a str>,
    err_label: &'a str,
) -> Result<Vec<Url>> {
    let mut all_urls: Vec<Url> = Vec::new();
    for s in primary.into_iter().chain(list) {
        let trimmed = s.trim();
        if trimmed.is_empty() {
            continue;
        }
        let url = Url::parse(trimmed)
            .with_context(|| format!("Invalid URL in {err_label}: {trimmed}"))?;
        if !all_urls.contains(&url) {
            all_urls.push(url);
        }
    }
    Ok(all_urls)
}

const CHAIN_ARG_DEFS: &[ChainArgDef] = &[
    ChainArgDef { name: "rpc-url", help: "RPC endpoint (repeat for failover)", append: true },
    ChainArgDef { name: "private-key", help: "Wallet key", append: false },
    ChainArgDef { name: "config-file", help: "Config override file", append: false },
    ChainArgDef { name: "market-address", help: "BoundlessMarket contract address", append: false },
    ChainArgDef {
        name: "set-verifier-address",
        help: "SetVerifier contract address",
        append: false,
    },
    ChainArgDef {
        name: "verifier-router-address",
        help: "VerifierRouter contract address",
        append: false,
    },
    ChainArgDef {
        name: "collateral-token-address",
        help: "Collateral token address",
        append: false,
    },
    ChainArgDef { name: "order-stream-url", help: "Order stream URL", append: false },
];

const CHAIN_ARGS_HELP: &str = "\
MULTI-CHAIN CONFIGURATION:
    Per-chain flags use the format --{option}-{chain_id}. Chain IDs are
    auto-discovered from the flags you provide.

    Example (two chains with failover):
      broker \\
        --rpc-url-1 https://eth.example.com \\
        --rpc-url-8453 https://base-primary.example.com \\
        --rpc-url-8453 https://base-backup.example.com \\
        --private-key-1 0xETH_KEY \\
        --private-key-8453 0xBASE_KEY

    Available per-chain flags:
      --rpc-url-{id}                    RPC endpoint (repeat for failover)
      --private-key-{id}                Wallet key
      --config-file-{id}                Config override file
      --market-address-{id}             BoundlessMarket contract address
      --set-verifier-address-{id}       SetVerifier contract address
      --verifier-router-address-{id}    VerifierRouter contract address
      --collateral-token-address-{id}   Collateral token address
      --order-stream-url-{id}           Order stream URL

    Env vars (PROVER_RPC_URL_{id}, PROVER_PRIVATE_KEY_{id}) also work.
    CLI flags take priority over env vars.";

#[derive(Debug, Default)]
struct PerChainArgs {
    rpc_urls: HashMap<u64, Vec<Url>>,
    private_keys: HashMap<u64, PrivateKeySigner>,
    config_files: HashMap<u64, PathBuf>,
    market_addresses: HashMap<u64, Address>,
    set_verifier_addresses: HashMap<u64, Address>,
    verifier_router_addresses: HashMap<u64, Address>,
    collateral_token_addresses: HashMap<u64, Address>,
    order_stream_urls: HashMap<u64, String>,
}

/// Scan raw CLI args and env vars to discover which chain IDs are referenced.
fn discover_chain_ids_from_argv() -> BTreeSet<u64> {
    let mut chain_ids = scan_chain_ids_from_args(std::env::args());
    chain_ids.extend(scan_chain_ids_from_env(std::env::vars()));
    chain_ids
}

/// Extract chain IDs referenced by `--{flag}-{chain_id}` CLI args.
///
/// Accepts both two-token (`--rpc-url-8453 URL`) and equals-joined
/// (`--rpc-url-8453=URL`) invocations; clap accepts either form, so the
/// discovery pass must too.
fn scan_chain_ids_from_args<I, S>(args: I) -> BTreeSet<u64>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let mut chain_ids = BTreeSet::new();
    for arg in args {
        let arg = arg.as_ref();
        for def in CHAIN_ARG_DEFS {
            let prefix = format!("--{}-", def.name);
            let Some(rest) = arg.strip_prefix(&prefix) else {
                continue;
            };
            let head = rest.split('=').next().unwrap_or(rest);
            if let Ok(chain_id) = head.parse::<u64>() {
                chain_ids.insert(chain_id);
            }
        }
    }
    chain_ids
}

/// Extract chain IDs referenced by `PROVER_RPC_URL[S]_{chain_id}` env vars.
fn scan_chain_ids_from_env<I, K, V>(vars: I) -> BTreeSet<u64>
where
    I: IntoIterator<Item = (K, V)>,
    K: AsRef<str>,
    V: AsRef<str>,
{
    let mut chain_ids = BTreeSet::new();
    for (key, value) in vars {
        if value.as_ref().trim().is_empty() {
            continue;
        }
        // Match either PROVER_RPC_URL_{chain_id} (primary) or
        // PROVER_RPC_URLS_{chain_id} (failover list). Try the longer prefix
        // first so PROVER_RPC_URLS_8453 doesn't get parsed as suffix "S_8453".
        let key = key.as_ref();
        let Some(suffix) =
            key.strip_prefix("PROVER_RPC_URLS_").or_else(|| key.strip_prefix("PROVER_RPC_URL_"))
        else {
            continue;
        };
        if let Ok(chain_id) = suffix.parse::<u64>() {
            chain_ids.insert(chain_id);
        }
    }
    chain_ids
}

/// Add dynamic clap args for a single chain ID.
fn register_chain_args(mut cmd: clap::Command, chain_id: u64) -> clap::Command {
    for def in CHAIN_ARG_DEFS {
        let arg_name: String = format!("{}-{chain_id}", def.name);
        let action = if def.append { ArgAction::Append } else { ArgAction::Set };
        cmd = cmd.arg(
            Arg::new(&arg_name)
                .long(arg_name)
                .action(action)
                .help(format!("{} for chain {chain_id}", def.help)),
        );
    }
    cmd
}

/// Extract per-chain values from clap matches into a PerChainArgs struct.
fn extract_per_chain_args(
    matches: &clap::ArgMatches,
    chain_ids: &BTreeSet<u64>,
) -> Result<PerChainArgs> {
    let mut per_chain = PerChainArgs::default();

    for &chain_id in chain_ids {
        if let Some(urls) = matches.get_many::<String>(&format!("rpc-url-{chain_id}")) {
            let parsed: Vec<Url> = urls
                .map(|s| {
                    Url::parse(s)
                        .with_context(|| format!("Invalid URL in --rpc-url-{chain_id}: {s}"))
                })
                .collect::<Result<_>>()?;
            per_chain.rpc_urls.insert(chain_id, parsed);
        }

        if let Some(key_str) = matches.get_one::<String>(&format!("private-key-{chain_id}")) {
            let key: PrivateKeySigner = key_str
                .parse()
                .with_context(|| format!("Invalid private key in --private-key-{chain_id}"))?;
            per_chain.private_keys.insert(chain_id, key);
        }

        if let Some(path_str) = matches.get_one::<String>(&format!("config-file-{chain_id}")) {
            per_chain.config_files.insert(chain_id, PathBuf::from(path_str));
        }

        if let Some(addr_str) = matches.get_one::<String>(&format!("market-address-{chain_id}")) {
            let addr: Address = addr_str
                .parse()
                .with_context(|| format!("Invalid address in --market-address-{chain_id}"))?;
            per_chain.market_addresses.insert(chain_id, addr);
        }

        if let Some(addr_str) =
            matches.get_one::<String>(&format!("set-verifier-address-{chain_id}"))
        {
            let addr: Address = addr_str
                .parse()
                .with_context(|| format!("Invalid address in --set-verifier-address-{chain_id}"))?;
            per_chain.set_verifier_addresses.insert(chain_id, addr);
        }

        if let Some(addr_str) =
            matches.get_one::<String>(&format!("verifier-router-address-{chain_id}"))
        {
            let addr: Address = addr_str.parse().with_context(|| {
                format!("Invalid address in --verifier-router-address-{chain_id}")
            })?;
            per_chain.verifier_router_addresses.insert(chain_id, addr);
        }

        if let Some(addr_str) =
            matches.get_one::<String>(&format!("collateral-token-address-{chain_id}"))
        {
            let addr: Address = addr_str.parse().with_context(|| {
                format!("Invalid address in --collateral-token-address-{chain_id}")
            })?;
            per_chain.collateral_token_addresses.insert(chain_id, addr);
        }

        if let Some(url_str) = matches.get_one::<String>(&format!("order-stream-url-{chain_id}")) {
            per_chain.order_stream_urls.insert(chain_id, url_str.clone());
        }
    }

    Ok(per_chain)
}

/// Two-pass CLI parsing: discover chain IDs from raw args, then build dynamic clap args.
fn parse_args() -> Result<(CoreArgs, PerChainArgs)> {
    let discovered_ids = discover_chain_ids_from_argv();
    let mut cmd = CoreArgs::command();
    cmd = cmd.after_long_help(CHAIN_ARGS_HELP);
    for &id in &discovered_ids {
        cmd = register_chain_args(cmd, id);
    }
    let matches = cmd.get_matches();
    let args = CoreArgs::from_arg_matches(&matches).unwrap_or_else(|e| e.exit());
    let per_chain = extract_per_chain_args(&matches, &discovered_ids)?;
    Ok((args, per_chain))
}

#[tokio::main]
async fn main() -> Result<()> {
    let (args, mut per_chain) = parse_args()?;

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

    let discovered_chains = discover_chains(&args, &mut per_chain)?;

    let mut chain_pipelines = Vec::new();
    let mut _config_watchers: Vec<ConfigWatcher> = Vec::new();

    for chain_args in &discovered_chains {
        let chain_config_watcher = ConfigWatcher::new_with_chain_override(
            &args.config_file,
            chain_args.config_override_path.as_deref(), // None with legacy single-chain config
            chain_args.chain_id,
        )
        .await
        .with_context(|| match chain_args.chain_id {
            Some(id) => format!("Failed to load config for chain {id}"),
            None => "Failed to load broker config".to_string(),
        })?;

        let config = chain_config_watcher.config.clone();
        let (provider, any_provider, gas_priority_mode) =
            build_chain_provider(&chain_args.rpc_urls, &chain_args.private_key, &args, &config)?;
        let provider = Arc::new(provider);

        let chain_id = provider.get_chain_id().await.context("Failed to get chain ID")?;
        if let Some(configured) = chain_args.chain_id {
            anyhow::ensure!(
                configured == chain_id,
                "Configured chain ID {configured} does not match RPC-reported chain ID {chain_id}. \
                 Check that the RPC URL for chain {configured} actually points at chain {configured}."
            );
        }

        let rpc_urls: Vec<&str> = chain_args.rpc_urls.iter().map(Url::as_str).collect();
        let provenance =
            if chain_args.chain_id.is_some() { "per-chain" } else { "legacy single-chain" };
        tracing::info!(chain_id, ?rpc_urls, provenance, "Starting pipeline for chain");
        let deployment = chain_args
            .deployment
            .clone()
            .map(Ok)
            .unwrap_or_else(|| resolve_deployment(args.deployment.as_ref(), chain_id))?;
        tracing::info!(chain_id, %deployment, "Using deployment configuration");

        let db_url = broker_sqlite_url_for_chain(&args.db_url, chain_id)
            .map_err(|e| anyhow::anyhow!("invalid broker database URL: {e}"))?;
        let db = Arc::new(
            SqliteDb::new(&db_url).await.context("Failed to open per-chain sqlite database")?,
        );

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
            db,
        });

        if chain_args.chain_id.is_some() {
            _config_watchers.push(chain_config_watcher);
        }
    }

    let broker = Broker::new(args, base_config_watcher).await?;

    broker.start_service(chain_pipelines).await.context("Broker service failed")?;

    Ok(())
}

/// Build per-chain Deployment objects from the PerChainArgs address fields.
/// Both market and set_verifier addresses are required per chain;
/// other fields (verifier_router, collateral_token, order_stream) are optional.
fn build_chain_deployments(
    per_chain: &PerChainArgs,
) -> Result<HashMap<u64, boundless_market::Deployment>> {
    let mut deployments = HashMap::new();
    let mut chain_ids: BTreeSet<u64> = BTreeSet::new();
    chain_ids.extend(per_chain.market_addresses.keys());
    chain_ids.extend(per_chain.set_verifier_addresses.keys());

    for chain_id in chain_ids {
        let market_address = per_chain.market_addresses.get(&chain_id).with_context(|| {
            format!(
                "--market-address-{chain_id} required (--set-verifier-address-{chain_id} was provided)"
            )
        })?;
        let set_verifier_address =
            per_chain
                .set_verifier_addresses
                .get(&chain_id)
                .with_context(|| {
                    format!(
                    "--set-verifier-address-{chain_id} required (--market-address-{chain_id} was provided)"
                )
                })?;

        let mut builder = boundless_market::Deployment::builder();
        builder
            .market_chain_id(chain_id)
            .boundless_market_address(*market_address)
            .set_verifier_address(*set_verifier_address);

        if let Some(addr) = per_chain.verifier_router_addresses.get(&chain_id) {
            builder.verifier_router_address(*addr);
        }
        if let Some(addr) = per_chain.collateral_token_addresses.get(&chain_id) {
            builder.collateral_token_address(*addr);
        }
        if let Some(url) = per_chain.order_stream_urls.get(&chain_id) {
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

/// Discover enabled chains from per-chain CLI args and environment variables.
///
/// Chains are discovered from two sources (CLI args take priority):
/// 1. `--rpc-url-{chain_id}` CLI args
/// 2. `PROVER_RPC_URL_{chain_id}` / `PROVER_RPC_URLS_{chain_id}` environment variables
///
/// If neither source yields anything, falls back to the legacy single-chain config
/// (`PROVER_RPC_URL` / `PROVER_RPC_URLS`, `PROVER_PRIVATE_KEY`, etc.) and returns a
/// single `ChainArgs` with `chain_id = None`; the actual ID is resolved by
/// `provider.get_chain_id()` at startup.
///
/// Errors if no RPC URL is configured anywhere.
fn discover_chains(args: &CoreArgs, per_chain: &mut PerChainArgs) -> Result<Vec<ChainArgs>> {
    let mut chain_urls: BTreeMap<u64, Vec<Url>> = BTreeMap::new();
    for (chain_id, urls) in per_chain.rpc_urls.drain() {
        chain_urls.insert(chain_id, urls);
    }

    // Discover chain IDs referenced by either PROVER_RPC_URL_{chain_id} or
    // PROVER_RPC_URLS_{chain_id}, then combine both forms per chain.
    let mut env_chain_ids: BTreeSet<u64> = BTreeSet::new();
    for (key, value) in std::env::vars() {
        if value.trim().is_empty() {
            continue;
        }
        let Some(suffix) =
            key.strip_prefix("PROVER_RPC_URLS_").or_else(|| key.strip_prefix("PROVER_RPC_URL_"))
        else {
            continue;
        };
        if let Ok(chain_id) = suffix.parse::<u64>() {
            env_chain_ids.insert(chain_id);
        }
    }

    for chain_id in env_chain_ids {
        if chain_urls.contains_key(&chain_id) {
            continue;
        }
        let primary_env = std::env::var(format!("PROVER_RPC_URL_{chain_id}")).ok();
        let extra_env = std::env::var(format!("PROVER_RPC_URLS_{chain_id}")).ok();
        let label = format!("PROVER_RPC_URL(S)_{chain_id}");
        let urls = merge_rpc_urls(
            primary_env.as_deref(),
            extra_env.as_deref().into_iter().flat_map(|s| s.split(',')),
            &label,
        )?;
        if !urls.is_empty() {
            chain_urls.insert(chain_id, urls);
        }
    }

    if chain_urls.is_empty() {
        // No per-chain configuration: synthesize a single legacy ChainArgs whose
        // actual chain_id will be resolved at startup via provider.get_chain_id().
        return Ok(vec![synthesize_legacy_chain(args)?]);
    }

    let mut chain_deployments = build_chain_deployments(per_chain)?;

    let global_private_key = args
        .private_key
        .clone()
        .or_else(|| std::env::var("PRIVATE_KEY").ok().and_then(|key| key.parse().ok()));

    let mut chains = Vec::with_capacity(chain_urls.len());
    for (chain_id, rpc_urls) in chain_urls {
        let private_key = per_chain
            .private_keys
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
                     Set --private-key-{chain_id} <key> or PROVER_PRIVATE_KEY_{chain_id} or PROVER_PRIVATE_KEY"
                )
            })?;

        let config_override_path = per_chain
            .config_files
            .remove(&chain_id)
            .or_else(|| ConfigWatcher::override_path_for_chain(&args.config_file, chain_id));

        let deployment = chain_deployments.remove(&chain_id);

        chains.push(ChainArgs {
            chain_id: Some(chain_id),
            rpc_urls,
            private_key,
            config_override_path,
            deployment,
        });
    }

    for (chain_id, path) in &per_chain.config_files {
        tracing::warn!(
            "--config-file-{chain_id} specified ({}) but no --rpc-url-{chain_id} found, ignoring",
            path.display()
        );
    }

    Ok(chains)
}

/// Build a single `ChainArgs` from the legacy single-chain CLI/env vars.
///
/// The `chain_id` is left as `None` — `main()` resolves it via `provider.get_chain_id()`
/// once the provider is built. Errors if no RPC URL or private key is configured.
fn synthesize_legacy_chain(args: &CoreArgs) -> Result<ChainArgs> {
    let mut rpc_urls = merge_rpc_urls(
        args.rpc_url.as_deref(),
        args.rpc_urls.iter().map(String::as_str),
        "PROVER_RPC_URL(S)",
    )?;

    // Legacy fallback: honor RPC_URL when PROVER_RPC_URL is unset.
    if rpc_urls.is_empty() {
        if let Ok(rpc_url_env) = std::env::var("RPC_URL") {
            let trimmed = rpc_url_env.trim();
            if !trimmed.is_empty() {
                let url = Url::parse(trimmed).context("Invalid RPC_URL environment variable")?;
                rpc_urls.push(url);
                tracing::info!("Using RPC_URL environment variable (PROVER_RPC_URL not set)");
            }
        }
    }

    if rpc_urls.is_empty() {
        anyhow::bail!(
            "No RPC URLs configured. Set one of: \
             PROVER_RPC_URL / PROVER_RPC_URLS (single-chain), \
             PROVER_RPC_URL_{{id}} / PROVER_RPC_URLS_{{id}} (per-chain env), \
             or --rpc-url-{{id}} (per-chain CLI)"
        );
    }

    let private_key = args
        .private_key
        .clone()
        .or_else(|| std::env::var("PRIVATE_KEY").ok().and_then(|key| key.parse().ok()))
        .context(
            "Private key not provided. Set PROVER_PRIVATE_KEY or PRIVATE_KEY environment variable",
        )?;

    Ok(ChainArgs {
        chain_id: None,
        rpc_urls,
        private_key,
        config_override_path: None,
        deployment: args.deployment.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use broker::CoreArgs;
    use std::path::PathBuf;

    const TEST_PRIVATE_KEY_0: &str =
        "0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80";
    const TEST_PRIVATE_KEY_1: &str =
        "0x59c6995e998f97a5a0044966f0945389dc9e86dae88c7a8412f4603b6b78690d";

    #[test]
    fn merge_rpc_urls_primary_is_first() {
        let extras = ["http://secondary.example.com", "http://tertiary.example.com"];
        let result =
            merge_rpc_urls(Some("http://primary.example.com"), extras, "test-label").unwrap();
        assert_eq!(result[0].as_str(), "http://primary.example.com/");
        assert_eq!(result.len(), 3);
    }

    #[test]
    fn merge_rpc_urls_deduplicates() {
        let extras = ["http://node.example.com"];
        let result = merge_rpc_urls(Some("http://node.example.com"), extras, "test-label").unwrap();
        assert_eq!(result.len(), 1);
    }

    #[test]
    fn merge_rpc_urls_skips_empty_entries() {
        let extras = ["", "   ", "http://node.example.com"];
        let result = merge_rpc_urls(Some(""), extras, "test-label").unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].as_str(), "http://node.example.com/");
    }

    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// Clear every env var that `synthesize_legacy_chain` / `discover_chains` consult,
    /// so legacy-path tests aren't affected by leakage from the developer's outer env.
    fn clear_legacy_env_vars() {
        std::env::remove_var("PROVER_RPC_URL");
        std::env::remove_var("PROVER_RPC_URLS");
        std::env::remove_var("PROVER_PRIVATE_KEY");
        std::env::remove_var("PRIVATE_KEY");
        std::env::remove_var("RPC_URL");
    }

    fn default_args() -> CoreArgs {
        CoreArgs {
            db_url: "sqlite::memory:".into(),
            rpc_url: None,
            rpc_urls: vec![],
            private_key: None,
            deployment: None,
            bento_api_url: None,
            bonsai_api_url: None,
            bonsai_api_key: None,
            multi_zkvm_endpoint: None,
            config_file: PathBuf::from("broker.toml"),
            deposit_amount: None,
            rpc_retry_max: 10,
            rpc_retry_backoff: 1000,
            rpc_retry_cu: 100,
            rpc_request_timeout: 15,
            log_json: false,
            listen_only: false,
            version_registry_address: None,
            force_version_check: false,
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
    fn discover_chains_errors_when_nothing_configured() {
        let _lock = ENV_LOCK.lock().unwrap();
        clear_chain_env_vars();
        clear_legacy_env_vars();

        let args = default_args();
        let mut per_chain = PerChainArgs::default();
        let err = discover_chains(&args, &mut per_chain).unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("No RPC URLs configured"), "expected URL-missing error, got: {msg}");
    }

    #[test]
    fn discover_chains_synthesizes_legacy_chain_from_args() {
        let _lock = ENV_LOCK.lock().unwrap();
        clear_chain_env_vars();
        clear_legacy_env_vars();

        let mut args = default_args();
        args.rpc_url = Some("http://primary.example.com".to_string());
        args.rpc_urls = vec!["http://backup.example.com".to_string()];
        args.private_key = Some(TEST_PRIVATE_KEY_0.parse().unwrap());

        let mut per_chain = PerChainArgs::default();
        let chains = discover_chains(&args, &mut per_chain).unwrap();
        assert_eq!(chains.len(), 1, "legacy synthesis should yield exactly one ChainArgs");
        assert_eq!(
            chains[0].chain_id, None,
            "legacy chain leaves chain_id unset; real ID is resolved at startup"
        );
        assert_eq!(chains[0].rpc_urls.len(), 2);
        assert_eq!(chains[0].rpc_urls[0].as_str(), "http://primary.example.com/");
        assert_eq!(chains[0].rpc_urls[1].as_str(), "http://backup.example.com/");
        assert_eq!(chains[0].private_key, TEST_PRIVATE_KEY_0.parse().unwrap());
    }

    #[test]
    fn discover_chains_legacy_falls_back_to_rpc_url_env() {
        // Legacy compat: when PROVER_RPC_URL is unset, RPC_URL is honored.
        let _lock = ENV_LOCK.lock().unwrap();
        clear_chain_env_vars();
        clear_legacy_env_vars();

        std::env::set_var("RPC_URL", "http://legacy-rpc.example.com");
        let mut args = default_args();
        args.private_key = Some(TEST_PRIVATE_KEY_0.parse().unwrap());

        let mut per_chain = PerChainArgs::default();
        let chains = discover_chains(&args, &mut per_chain).unwrap();
        assert_eq!(chains.len(), 1);
        assert_eq!(chains[0].chain_id, None);
        assert_eq!(chains[0].rpc_urls[0].as_str(), "http://legacy-rpc.example.com/");

        clear_legacy_env_vars();
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

        let mut per_chain = PerChainArgs::default();
        let chains = discover_chains(&args, &mut per_chain).unwrap();
        assert_eq!(chains.len(), 2);
        assert_eq!(chains[0].chain_id, Some(1));
        assert_eq!(chains[0].rpc_urls[0].as_str(), "http://eth.example.com/");
        assert_eq!(chains[1].chain_id, Some(8453));
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

        let mut per_chain = PerChainArgs::default();
        let chains = discover_chains(&args, &mut per_chain).unwrap();
        assert_eq!(chains[0].chain_id, Some(1));
        assert_eq!(chains[0].private_key, chain_key.parse().unwrap());
        assert_eq!(chains[1].chain_id, Some(8453));
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

        let mut per_chain = PerChainArgs::default();
        let chains = discover_chains(&args, &mut per_chain).unwrap();
        assert_eq!(chains.len(), 1);
        assert_eq!(chains[0].rpc_urls.len(), 3);
        assert_eq!(chains[0].rpc_urls[0].as_str(), "http://primary.example.com/");
        assert_eq!(chains[0].rpc_urls[1].as_str(), "http://backup1.example.com/");
        assert_eq!(chains[0].rpc_urls[2].as_str(), "http://backup2.example.com/");

        clear_chain_env_vars();
    }

    #[test]
    fn discover_chains_failover_only_registers_chain() {
        // Regression: setting only PROVER_RPC_URLS_{chain_id} (no singular
        // PROVER_RPC_URL_{chain_id}) should still register the chain, mirroring
        // the v1.x single-chain semantics where PROVER_RPC_URLS alone was valid.
        let _lock = ENV_LOCK.lock().unwrap();
        clear_chain_env_vars();

        std::env::set_var(
            "PROVER_RPC_URLS_8453",
            "http://backup1.example.com,http://backup2.example.com",
        );

        let mut args = default_args();
        args.private_key = Some(TEST_PRIVATE_KEY_0.parse().unwrap());

        let mut per_chain = PerChainArgs::default();
        let chains = discover_chains(&args, &mut per_chain).unwrap();
        assert_eq!(chains.len(), 1);
        assert_eq!(chains[0].chain_id, Some(8453));
        assert_eq!(chains[0].rpc_urls.len(), 2);
        assert_eq!(chains[0].rpc_urls[0].as_str(), "http://backup1.example.com/");
        assert_eq!(chains[0].rpc_urls[1].as_str(), "http://backup2.example.com/");

        clear_chain_env_vars();
    }

    #[test]
    fn discover_chain_ids_from_failover_only_env() {
        let _lock = ENV_LOCK.lock().unwrap();
        clear_chain_env_vars();

        std::env::set_var("PROVER_RPC_URLS_167000", "http://taiko1.example.com");

        let ids = discover_chain_ids_from_argv();
        assert!(ids.contains(&167000));

        clear_chain_env_vars();
    }

    #[test]
    fn scan_chain_ids_from_args_handles_space_form() {
        let argv = ["broker", "--rpc-url-8453", "https://base.example.com"];
        let ids = scan_chain_ids_from_args(argv);
        assert!(ids.contains(&8453), "space-separated --rpc-url-{{id}} form must register chain");
    }

    #[test]
    fn scan_chain_ids_from_args_handles_equals_form() {
        // Regression: previously `--rpc-url-8453=URL` was parsed as suffix "8453=URL"
        // and failed u64::parse, so the chain was never registered. clap then errored
        // with "unexpected argument '--rpc-url-8453' found".
        let argv = ["broker", "--rpc-url-8453=https://base.example.com"];
        let ids = scan_chain_ids_from_args(argv);
        assert!(ids.contains(&8453), "--rpc-url-{{id}}=URL form must register chain");
    }

    #[test]
    fn scan_chain_ids_from_args_picks_up_multiple_chains_and_flags() {
        let argv = [
            "broker",
            "--rpc-url-1=https://eth.example.com",
            "--private-key-8453",
            "0xKEY",
            "--market-address-167000=0xMARKET",
        ];
        let ids = scan_chain_ids_from_args(argv);
        assert!(ids.contains(&1));
        assert!(ids.contains(&8453));
        assert!(ids.contains(&167000));
        assert_eq!(ids.len(), 3);
    }

    #[test]
    fn scan_chain_ids_from_args_ignores_non_numeric_suffix() {
        let argv = ["broker", "--rpc-url-foo=URL", "--unrelated-8453"];
        let ids = scan_chain_ids_from_args(argv);
        assert!(ids.is_empty());
    }

    #[test]
    fn discover_chains_explicit_chain_config_file() {
        let _lock = ENV_LOCK.lock().unwrap();
        clear_chain_env_vars();

        std::env::set_var("PROVER_RPC_URL_8453", "http://base.example.com");

        let mut args = default_args();
        args.private_key = Some(TEST_PRIVATE_KEY_0.parse().unwrap());

        let mut per_chain = PerChainArgs::default();
        per_chain.config_files.insert(8453, PathBuf::from("/custom/path/broker.base.toml"));

        let chains = discover_chains(&args, &mut per_chain).unwrap();
        assert_eq!(chains.len(), 1);
        assert_eq!(
            chains[0].config_override_path,
            Some(PathBuf::from("/custom/path/broker.base.toml"))
        );

        clear_chain_env_vars();
    }
}
