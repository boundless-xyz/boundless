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

use alloy::{
    primitives::utils::parse_ether,
    providers::{fillers::ChainIdFiller, network::EthereumWallet, ProviderBuilder, WalletProvider},
    rpc::client::RpcClient,
    transports::{
        http::Http,
        layers::{FallbackLayer, RetryBackoffLayer},
    },
};
use anyhow::{Context, Result};
use boundless_market::{
    balance_alerts_layer::{BalanceAlertConfig, BalanceAlertLayer},
    contracts::boundless_market::BoundlessMarketService,
    dynamic_gas_filler::DynamicGasFiller,
    nonce_layer::NonceProvider,
};
use broker::{config::ConfigWatcher, Args, Broker, CustomRetryPolicy};
use clap::Parser;
use tower::ServiceBuilder;
use tracing_subscriber::fmt::format::FmtSpan;
use url::Url;

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    let all_rpc_urls = collect_rpc_urls(args.rpc_url.clone(), args.rpc_urls.clone())?;

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

    // Build RPC client with fallback support if multiple URLs are provided
    let client = if all_rpc_urls.len() > 1 {
        // Multiple URLs - use fallback transport
        let transports: Vec<Http<_>> =
            all_rpc_urls.iter().map(|url| Http::new(url.clone())).collect();

        let active_count =
            std::num::NonZeroUsize::new(transports.len()).unwrap_or(std::num::NonZeroUsize::MIN);
        let fallback_layer = FallbackLayer::default().with_active_transport_count(active_count);

        tracing::info!(
            "Configuring broker with fallback RPC support: {} URLs: {:?}",
            all_rpc_urls.len(),
            all_rpc_urls
        );

        let transport =
            ServiceBuilder::new().layer(retry_layer).layer(fallback_layer).service(transports);

        RpcClient::builder().transport(transport, false)
    } else {
        // Single URL - use regular provider
        let single_url = &all_rpc_urls[0];
        tracing::info!("Configuring broker with single RPC URL: {}", single_url);
        RpcClient::builder().layer(retry_layer).http(single_url.clone())
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
        .connect_client(client);

    let provider = NonceProvider::new(base_provider, wallet.clone());
    let broker =
        Broker::new(args.clone(), provider.clone(), config_watcher, gas_priority_mode).await?;

    // TODO: Move this code somewhere else / monitor our balanceOf and top it up as needed
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

    // Await broker shutdown before returning from main
    broker.start_service().await.context("Broker service failed")?;

    Ok(())
}

/// Collect and parse all RPC URLs from args and environment variables
/// Returns a deduplicated list of valid RPC URLs
fn collect_rpc_urls(rpc_url: Option<String>, rpc_urls: Vec<String>) -> Result<Vec<Url>> {
    let mut all_rpc_urls = std::collections::HashSet::new();

    // Parse PROVER_RPC_URL (ignore if empty)
    if let Some(url_str) = rpc_url {
        if !url_str.is_empty() {
            let url =
                Url::parse(&url_str).context("Invalid PROVER_RPC_URL environment variable")?;
            all_rpc_urls.insert(url);
        }
    }

    // Parse PROVER_RPC_URLS (ignore empty strings, split by comma)
    for url_str in rpc_urls {
        let url_str = url_str.trim();
        if !url_str.is_empty() {
            let url =
                Url::parse(url_str).context("Invalid PROVER_RPC_URLS environment variable")?;
            all_rpc_urls.insert(url);
        }
    }

    // Backward compatibility: check RPC_URL if no URLs collected yet
    if all_rpc_urls.is_empty() {
        if let Ok(legacy_rpc_url) = std::env::var("RPC_URL") {
            if !legacy_rpc_url.is_empty() {
                let url =
                    Url::parse(&legacy_rpc_url).context("Invalid RPC_URL environment variable")?;
                all_rpc_urls.insert(url);
                tracing::info!("Using RPC_URL environment variable (PROVER_RPC_URL not set)");
            }
        }
    }

    // Error early if no RPC URLs provided
    if all_rpc_urls.is_empty() {
        anyhow::bail!(
            "No RPC URLs provided. Please set at least one using PROVER_RPC_URL or PROVER_RPC_URLS environment variables"
        );
    }

    Ok(all_rpc_urls.into_iter().collect())
}
