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

//! Requestor-side pricing checks for proof requests.
//!
//! This module provides utilities for requestors to check if their proof requests
//! are likely to be accepted by provers before submitting them to the market.
//! It uses the same pricing logic that provers use, but with minimal defaults
//! appropriate for requestor-side checks.

use std::collections::HashSet;
use std::sync::Arc;

use alloy::network::Ethereum;
use alloy::primitives::utils::format_ether;
use alloy::primitives::{Address, Bytes, U256};
use alloy::providers::Provider;
use alloy::uint;
use anyhow::Context;
use moka::future::Cache;
use moka::policy::EvictionPolicy;

use super::local_executor::LocalExecutor;
use super::prover::ProverObj;
use super::{
    FulfillmentType, MarketConfig, OrderPricingContext, OrderPricingError, OrderPricingOutcome,
    OrderRequest, PreflightCache,
};
use crate::contracts::boundless_market::BoundlessMarketService;
use crate::contracts::ProofRequest;
use crate::price_oracle::{Amount, Asset, PriceOracleManager};
use crate::price_provider::PriceProviderArc;
use crate::selector::SupportedSelectors;
use crate::storage::{StandardDownloader, StorageDownloader};

const ONE_MILLION: U256 = uint!(1_000_000_U256);

/// Price a request using the broker pricing logic with minimal defaults.
///
/// This checks both LockAndFulfill and FulfillAfterLockExpire fulfillment types
/// and logs warnings if the request may not be accepted by provers.
///
/// The `executor` parameter should be the same executor used during preflight
/// execution, allowing cached results to be reused.
///
/// If a `price_provider` is provided, the p10 (10th percentile) market price will be
/// used for `min_mcycle_price` to give a more accurate estimate of whether provers
/// will accept the request. If not provided or if fetching fails, falls back to
/// `MarketConf::default()`.
pub async fn requestor_order_preflight<P>(
    request: ProofRequest,
    executor: LocalExecutor,
    provider: Arc<P>,
    market_address: Address,
    chain_id: u64,
    price_provider: Option<PriceProviderArc>,
    price_oracle: Option<Arc<PriceOracleManager>>,
) -> anyhow::Result<Option<u64>>
where
    P: Provider<Ethereum> + 'static + Clone,
{
    tracing::debug!("Checking order preflight");
    // Caller is unused with RequestorPricingContext execution, so it's safe to use Address::ZERO
    let market =
        BoundlessMarketService::new_for_broker(market_address, provider.clone(), Address::ZERO);
    let collateral_token_decimals = market
        .collateral_token_decimals()
        .await
        .context("Failed to fetch collateral token decimals")?;

    // Create preflight cache - the LocalExecutor handles execution deduplication internally
    let preflight_cache: PreflightCache =
        Arc::new(Cache::builder().eviction_policy(EvictionPolicy::lru()).max_capacity(32).build());

    // Build market config, using price provider if available
    let market_config = build_market_config(price_provider).await;

    // Create a standard downloader for fetching images and inputs
    let downloader = Arc::new(StandardDownloader::new().await);

    let pricing_context = RequestorPricingContext::new(
        provider.clone(),
        collateral_token_decimals,
        market_config,
        preflight_cache,
        Arc::new(executor),
        downloader,
        price_oracle,
    );

    let mut result_cycle_count = None;

    // Empty signature bytes - pricing logic does not validate the signature,
    // it only examines request parameters (cycle counts, bid, deadline, etc.)
    let client_sig = Bytes::new();

    // Price for LockAndFulfill
    let mut lock_order = Box::new(OrderRequest::new(
        request.clone(),
        client_sig.clone(),
        FulfillmentType::LockAndFulfill,
        market_address,
        chain_id,
    ));

    let lock_outcome = pricing_context
        .price_order(&mut lock_order)
        .await
        .context("Requestor price_order (LockAndFulfill) failed")?;

    // Check if LockAndFulfill pricing resulted in Lock
    match &lock_outcome {
        OrderPricingOutcome::Lock { total_cycles, .. } => {
            result_cycle_count = Some(*total_cycles);
        }
        OrderPricingOutcome::Skip { reason } => {
            tracing::warn!("Request may not be accepted to be locked by default: {}", reason);
        }
        OrderPricingOutcome::ProveAfterLockExpire { .. } => {
            tracing::warn!(
                "Request priced as ProveAfterLockExpire when LockAndFulfill was expected"
            );
        }
    };

    // Price for FulfillAfterLockExpire
    let mut expire_order = Box::new(OrderRequest::new(
        request,
        client_sig,
        FulfillmentType::FulfillAfterLockExpire,
        market_address,
        chain_id,
    ));

    let expire_outcome = pricing_context
        .price_order(&mut expire_order)
        .await
        .context("Requestor price_order (FulfillAfterLockExpire) failed")?;

    // Check if FulfillAfterLockExpire pricing resulted in ProveAfterLockExpire
    match &expire_outcome {
        OrderPricingOutcome::ProveAfterLockExpire { total_cycles, .. } => {
            result_cycle_count = Some(*total_cycles);
        }
        OrderPricingOutcome::Skip { reason } => {
            tracing::warn!("Request may not be accepted for secondary fulfillment: {}", reason);
        }
        OrderPricingOutcome::Lock { .. } => {
            tracing::warn!("Request priced as Lock when FulfillAfterLockExpire was expected");
        }
    };

    tracing::debug!("Finished checking preflight");

    Ok(result_cycle_count)
}

async fn build_market_config(price_provider: Option<PriceProviderArc>) -> MarketConfig {
    let Some(provider) = price_provider else {
        return MarketConfig::default();
    };

    let min_mcycle_price = match provider.price_percentiles().await {
        Ok(percentiles) => percentiles.p10.saturating_mul(ONE_MILLION),
        Err(e) => {
            tracing::warn!("Failed to fetch market prices for preflight, using 0 eth/mcycle: {e}",);
            U256::ZERO
        }
    };

    tracing::debug!("Using market price for preflight: {}", format_ether(min_mcycle_price));
    MarketConfig {
        min_mcycle_price: Amount::new(min_mcycle_price, Asset::ETH),
        // Note: collateral cycle price is not available, so ignore this price check during preflight
        min_mcycle_price_collateral_token: Amount::new(U256::ZERO, Asset::ZKC),
        ..MarketConfig::default()
    }
}

/// Pricing context for requestor-side checks.
///
/// This implements [OrderPricingContext] with minimal defaults appropriate for
/// requestor-side pricing checks. It skips broker-specific checks like balance
/// verification and requestor allowlists.
pub struct RequestorPricingContext<P> {
    provider: Arc<P>,
    prover: ProverObj,
    market_config: MarketConfig,
    supported_selectors: SupportedSelectors,
    preflight_cache: PreflightCache,
    collateral_token_decimals: u8,
    downloader: Arc<StandardDownloader>,
    price_oracle: Option<Arc<PriceOracleManager>>,
}

impl<P> RequestorPricingContext<P>
where
    P: Provider<Ethereum> + 'static + Clone,
{
    /// Create a new requestor pricing context.
    pub fn new(
        provider: Arc<P>,
        collateral_token_decimals: u8,
        market_config: MarketConfig,
        preflight_cache: PreflightCache,
        prover: ProverObj,
        downloader: Arc<StandardDownloader>,
        price_oracle: Option<Arc<PriceOracleManager>>,
    ) -> Self {
        Self {
            provider,
            prover,
            market_config,
            supported_selectors: SupportedSelectors::default(),
            preflight_cache,
            collateral_token_decimals,
            downloader,
            price_oracle,
        }
    }
}

impl<P> OrderPricingContext for RequestorPricingContext<P>
where
    P: Provider<Ethereum> + 'static + Clone,
{
    fn market_config(&self) -> Result<MarketConfig, OrderPricingError> {
        Ok(self.market_config.clone())
    }

    fn supported_selectors(&self) -> &SupportedSelectors {
        &self.supported_selectors
    }

    fn preflight_cache(&self) -> &PreflightCache {
        &self.preflight_cache
    }

    fn collateral_token_decimals(&self) -> u8 {
        self.collateral_token_decimals
    }

    fn check_requestor_allowed(
        &self,
        _order: &OrderRequest,
        _denied_addresses_opt: Option<&HashSet<Address>>,
    ) -> Result<Option<OrderPricingOutcome>, OrderPricingError> {
        Ok(None)
    }

    async fn check_request_available(
        &self,
        _order: &OrderRequest,
    ) -> Result<Option<OrderPricingOutcome>, OrderPricingError> {
        // Assume request is available for requestors
        Ok(None)
    }

    #[cfg(feature = "prover_utils")]
    async fn estimate_gas_to_fulfill_pending(&self) -> Result<u64, OrderPricingError> {
        Ok(0)
    }

    fn is_priority_requestor(&self, _client_addr: &Address) -> bool {
        false
    }

    async fn check_available_balances(
        &self,
        _order: &OrderRequest,
        _order_gas_cost: U256,
        _lock_expired: bool,
        _lockin_collateral: U256,
    ) -> Result<Option<OrderPricingOutcome>, OrderPricingError> {
        Ok(None)
    }

    async fn current_gas_price(&self) -> Result<u128, OrderPricingError> {
        self.provider
            .get_gas_price()
            .await
            .map_err(|err| OrderPricingError::RpcErr(Arc::new(err.into())))
    }

    async fn convert_to_eth(&self, amount: &Amount) -> Result<Amount, OrderPricingError> {
        if amount.asset == Asset::ETH {
            return Ok(amount.clone());
        }
        let Some(oracle) = self.price_oracle.as_ref() else {
            tracing::warn!(
                "Skipping price check: conversion from {} to ETH requires a price oracle (configure one for accurate preflight)",
                amount.asset
            );
            return Ok(Amount::new(U256::ZERO, Asset::ETH));
        };
        oracle.convert(amount, Asset::ETH).await.map_err(|e| {
            OrderPricingError::UnexpectedErr(Arc::new(anyhow::anyhow!(
                "Failed to convert {} to ETH: {}",
                amount,
                e
            )))
        })
    }

    async fn convert_to_zkc(&self, amount: &Amount) -> Result<Amount, OrderPricingError> {
        if amount.asset == Asset::ZKC {
            return Ok(amount.clone());
        }
        let Some(oracle) = self.price_oracle.as_ref() else {
            tracing::warn!(
                "Skipping price check: conversion from {} to ZKC requires a price oracle (configure one for accurate preflight)",
                amount.asset
            );
            return Ok(Amount::new(U256::MAX, Asset::ZKC));
        };
        oracle.convert(amount, Asset::ZKC).await.map_err(|e| {
            OrderPricingError::UnexpectedErr(Arc::new(anyhow::anyhow!(
                "Failed to convert {} to ZKC: {}",
                amount,
                e
            )))
        })
    }

    fn prover(&self) -> &ProverObj {
        &self.prover
    }

    fn downloader(&self) -> Arc<dyn StorageDownloader + Send + Sync> {
        self.downloader.clone()
    }
}
