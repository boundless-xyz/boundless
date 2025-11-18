// Copyright 2025 Boundless Foundation, Inc.
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
    transports::layers::RetryBackoffLayer,
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
use tracing_subscriber::fmt::format::FmtSpan;
use url::Url;

#[tokio::main]
async fn main() -> Result<()> {
    let mut args = Args::parse();

    // Backward compatibility: allow RPC_URL when PROVER_RPC_URL is not set
    if std::env::var("PROVER_RPC_URL").is_err() {
        if let Ok(legacy_rpc_url) = std::env::var("RPC_URL") {
            args.rpc_url =
                Url::parse(&legacy_rpc_url).context("Invalid RPC_URL environment variable")?;
            tracing::info!("Using RPC_URL environment variable (PROVER_RPC_URL not set)");
        }
    }
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
    args.private_key = Some(private_key.clone());

    let wallet = EthereumWallet::from(private_key.clone());

    let retry_layer = RetryBackoffLayer::new_with_policy(
        args.rpc_retry_max,
        args.rpc_retry_backoff,
        args.rpc_retry_cu,
        CustomRetryPolicy,
    );
    let client = RpcClient::builder().layer(retry_layer).http(args.rpc_url.clone());

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
        config.market.gas_priority_mode
    };
    let dynamic_gas_filler = DynamicGasFiller::new(
        20, // 20% increase of gas limit
        priority_mode,
        wallet.default_signer().address(),
    );

    let base_provider = ProviderBuilder::new()
        .disable_recommended_fillers()
        .filler(ChainIdFiller::default())
        .filler(dynamic_gas_filler)
        .layer(balance_alerts_layer)
        .connect_client(client);

    let provider = NonceProvider::new(base_provider, wallet.clone());
    let broker = Broker::new(args.clone(), provider.clone(), config_watcher).await?;

    // TODO: Move this code somewhere else / monitor our balanceOf and top it up as needed
    if let Some(deposit_amount) = args.deposit_amount.as_ref() {
        let boundless_market = BoundlessMarketService::new(
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
