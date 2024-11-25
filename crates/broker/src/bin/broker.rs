// Copyright (c) 2024 RISC Zero, Inc.
//
// All rights reserved.

use alloy::providers::{network::EthereumWallet, ProviderBuilder, WalletProvider};
use alloy_chains::NamedChain;
use anyhow::{Context, Result};
use boundless_market::contracts::proof_market::BoundlessMarketService;
use broker::{Args, Broker};
use clap::Parser;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let args = Args::try_parse()?;

    let wallet = EthereumWallet::from(args.private_key.clone());

    let provider = ProviderBuilder::new()
        .with_recommended_fillers()
        .wallet(wallet)
        .with_chain(NamedChain::Sepolia)
        .on_http(args.rpc_url.clone());

    // TODO: Move this code somewhere else / monitor our balanceOf and top it up as needed
    if let Some(deposit_amount) = args.deposit_amount.as_ref() {
        let proof_market = BoundlessMarketService::new(
            args.proof_market_addr,
            provider.clone(),
            provider.default_signer_address(),
        );

        let amount = alloy::primitives::utils::parse_ether(&deposit_amount).unwrap();
        tracing::info!("pre-depositing {deposit_amount} ETH into the market contract");
        proof_market.deposit(amount).await.context("Failed to deposit to market")?;
    }

    let broker = Broker::new(args, provider).await?;

    broker.start_service().await.context("Broker service failed")?;

    Ok(())
}
