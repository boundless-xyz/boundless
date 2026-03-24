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
    sequential_fallback::SequentialFallbackLayer, Args, Broker, CustomRetryPolicy,
};
use clap::Parser;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;
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

    let (tx_priority_mode, estimation_priority_mode) = {
        let config = config_watcher.config.lock_all().context("Failed to read config")?;
        (
            config.market.gas_priority_mode.clone(),
            config.market.gas_estimation_priority_mode.clone(),
        )
    };
    let dynamic_gas_filler = DynamicGasFiller::new(
        20, // 20% increase of gas limit
        tx_priority_mode,
        wallet.default_signer().address(),
    );

    // Clone the tx priority_mode Arc so we can pass it to the broker for runtime updates.
    // Create a separate Arc for the estimation priority mode used by ChainMonitorService.
    let gas_priority_mode = dynamic_gas_filler.priority_mode.clone();
    let gas_estimation_priority_mode = Arc::new(RwLock::new(estimation_priority_mode));

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
        gas_estimation_priority_mode,
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
