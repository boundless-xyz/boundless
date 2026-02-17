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

//! Helper functions to interact with the price oracle within the CLI.

use crate::display::DisplayManager;
use alloy_chains::NamedChain;
use anyhow::Context;
use boundless_market::price_oracle::{
    Amount, Asset, PriceOracleConfig, PriceOracleManager, TradingPair,
};
use inquire::Text;
use std::sync::Arc;
use url::Url;

/// Validate and normalize amount input using the same validation as config parsing
pub fn validate_amount_input(
    input: &str,
    allowed_assets: &[Asset],
    field_name: &str,
) -> anyhow::Result<String> {
    Amount::parse_with_allowed(input, allowed_assets, None)
        .map(|_| input.trim().to_string())
        .map_err(|e| {
            anyhow::anyhow!(
                "Invalid {} format: {}. Expected format: '<value> <asset>' where asset is one of: {}",
                field_name,
                e,
                allowed_assets.iter().map(|a| format!("{:?}", a)).collect::<Vec<_>>().join(", ")
            )
        })
}

/// Prompt for amount with validation and retry on invalid input
pub fn prompt_validated_amount(
    prompt_text: &str,
    default: &str,
    help_message: &str,
    allowed_assets: &[Asset],
    field_name: &str,
) -> anyhow::Result<String> {
    loop {
        let input = Text::new(prompt_text)
            .with_default(default)
            .with_help_message(help_message)
            .prompt()
            .context("Failed to get user input")?;

        match validate_amount_input(&input, allowed_assets, field_name) {
            Ok(validated) => return Ok(validated),
            Err(e) => {
                eprintln!("Error: {}", e);
                eprintln!("Please try again.\n");
            }
        }
    }
}

/// Try to initialize a price oracle for dual-currency display
/// Returns None if price fetching fails (display will fall back to single currency)
pub async fn try_init_price_oracle(
    rpc_url: &Url,
    chain_id: u64,
) -> anyhow::Result<Arc<PriceOracleManager>, anyhow::Error> {
    let provider = alloy::providers::ProviderBuilder::new().connect(rpc_url.as_ref()).await?;

    // Build price oracle from default config (uses Chainlink on-chain)
    let config = PriceOracleConfig::default();
    let named_chain = NamedChain::try_from(chain_id)?;

    Ok(Arc::new(config.build(named_chain, provider)?))
}

/// Fetch and display current market prices for ETH/USD and ZKC/USD, returning an error if fetching fails
pub async fn fetch_and_display_prices(
    oracle: Arc<PriceOracleManager>,
    display: &DisplayManager,
) -> anyhow::Result<()> {
    display.status("Status", "Fetching current conversion rates...", "yellow");

    oracle.refresh_all_rates().await;

    // Fetch initial prices
    let (eth_result, zkc_result) =
        tokio::join!(oracle.get_rate(TradingPair::EthUsd), oracle.get_rate(TradingPair::ZkcUsd),);

    match (eth_result, zkc_result) {
        (Ok(eth_quote), Ok(zkc_quote)) => {
            display.item_colored("ETH/USD", format!("${:.2}", eth_quote.rate_to_f64()), "cyan");
            display.item_colored("ZKC/USD", format!("${:.4}", zkc_quote.rate_to_f64()), "cyan");

            Ok(())
        }
        _ => Err(anyhow::anyhow!("⚠  Could not fetch prices from price oracle")),
    }
}

/// Format an amount with its USD equivalent (or native equivalent for USD amounts)
pub async fn format_amount_with_conversion(
    amount_str: &str,
    target: Option<Asset>,
    price_oracle: Option<Arc<PriceOracleManager>>,
) -> String {
    let Ok(amount) = Amount::parse(amount_str, None) else {
        return amount_str.to_string();
    };

    let Some(oracle) = price_oracle else {
        return amount_str.to_string();
    };

    match amount.asset {
        Asset::ETH | Asset::ZKC => {
            if let Ok(usd) = oracle.convert(&amount, Asset::USD).await {
                format!("{} (~{:.6})", amount_str, usd)
            } else {
                amount_str.to_string()
            }
        }
        Asset::USD => {
            if let Some(target_asset) = target {
                if let Ok(converted) = oracle.convert(&amount, target_asset).await {
                    return format!(
                        "{:.6} (~{} currently, converted at runtime)",
                        amount_str, converted
                    );
                }
            }

            // No target specified or conversion failed, just indicate runtime conversion
            format!("{} (converted at runtime)", amount_str)
        }
    }
}

/// Try to convert an amount to USD for dual-currency display, returning original string on failure
pub async fn try_convert_to_usd(
    amount_str: &str,
    price_oracle: Option<Arc<PriceOracleManager>>,
) -> String {
    let Ok(amount) = Amount::parse(amount_str, None) else {
        return amount_str.to_string();
    };

    let Some(oracle) = price_oracle else {
        return amount_str.to_string();
    };

    match amount.asset {
        Asset::ETH | Asset::ZKC => {
            if let Ok(usd) = oracle.convert(&amount, Asset::USD).await {
                format!("{:.6}", usd)
            } else {
                amount_str.to_string()
            }
        }
        Asset::USD => amount_str.to_string(),
    }
}
