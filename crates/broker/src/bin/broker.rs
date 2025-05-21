// Copyright (c) 2025 RISC Zero, Inc.
//
// All rights reserved.

use alloy::{
    primitives::utils::parse_ether,
    providers::{network::EthereumWallet, ProviderBuilder, WalletProvider},
    rpc::client::RpcClient,
    transports::layers::RetryBackoffLayer,
};
use alloy_chains::NamedChain;
use anyhow::{Context, Result};
use boundless_market::{
    balance_alerts_layer::{BalanceAlertConfig, BalanceAlertLayer},
    contracts::boundless_market::BoundlessMarketService,
};
use broker::{Args, Broker, Config, CustomRetryPolicy};
use clap::Parser;
use tracing_subscriber::fmt::format::FmtSpan;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_span_events(FmtSpan::CLOSE)
        .init();

    let args = Args::parse();
    let config = Config::load(&args.config_file).await?;

    let wallet = EthereumWallet::from(args.private_key.clone());

    let retry_layer = RetryBackoffLayer::new_with_policy(
        args.rpc_retry_max,
        args.rpc_retry_backoff,
        args.rpc_retry_cu,
        CustomRetryPolicy,
    );
    let client = RpcClient::builder().layer(retry_layer).http(args.rpc_url.clone());
    let balance_alerts_layer = BalanceAlertLayer::new(BalanceAlertConfig {
        watch_address: wallet.default_signer().address(),
        warn_threshold: config
            .market
            .balance_warn_threshold
            .map(|s| parse_ether(&s))
            .transpose()?,
        error_threshold: config
            .market
            .balance_error_threshold
            .map(|s| parse_ether(&s))
            .transpose()?,
    });

    let provider = ProviderBuilder::new()
        .with_cached_nonce_management()
        .layer(balance_alerts_layer)
        .wallet(wallet)
        .with_chain(NamedChain::Sepolia)
        .connect_client(client);

    // TODO: Move this code somewhere else / monitor our balanceOf and top it up as needed
    if let Some(deposit_amount) = args.deposit_amount.as_ref() {
        let boundless_market = BoundlessMarketService::new(
            args.boundless_market_address,
            provider.clone(),
            provider.default_signer_address(),
        );

        tracing::info!("pre-depositing {deposit_amount} HP into the market contract");
        boundless_market
            .deposit_stake_with_permit(*deposit_amount, &args.private_key)
            .await
            .context("Failed to deposit to market")?;
    }

    let broker = Broker::new(args, provider).await?;

    broker.start_service().await.context("Broker service failed")?;

    Ok(())
}
