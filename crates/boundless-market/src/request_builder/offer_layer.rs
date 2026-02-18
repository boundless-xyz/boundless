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

use super::{Adapt, Layer, MissingFieldError, RequestParams};
use crate::{
    contracts::{Offer, PredicateType, RequestId, Requirements},
    dynamic_gas_filler::PriorityMode,
    price_oracle::{Amount, Asset, PriceOracleManager},
    price_provider::PriceProviderArc,
    prover_utils::config_defaults::max_journal_bytes,
    request_builder::ParameterizationMode,
    selector::{ProofType, SupportedSelectors},
    util::now_timestamp,
    LARGE_REQUESTOR_LIST_THRESHOLD_KHZ, XL_REQUESTOR_LIST_THRESHOLD_KHZ,
};
use alloy::{
    network::Ethereum,
    primitives::{
        utils::{format_units, Unit},
        U256,
    },
    providers::Provider,
};
use anyhow::{bail, Context, Result};
use clap::Args;
use derive_builder::Builder;
use std::sync::Arc;

/// Convert a price Amount to ETH (wei).
///
/// Returns the value directly if the amount is already in ETH.
/// Converts from USD to ETH using the price oracle if needed.
/// Returns an error if:
/// - The amount is in USD but no price oracle is provided
/// - The amount is in an invalid asset (ZKC not allowed for prices)
async fn convert_amount_to_eth(
    amount: &Amount,
    oracle: Option<&PriceOracleManager>,
) -> Result<U256> {
    match amount.asset {
        Asset::ETH => Ok(amount.value),
        Asset::USD => {
            let oracle = oracle.context(
                "Price oracle required to convert USD prices to ETH. \
                 Configure a price oracle or specify prices in ETH.",
            )?;
            let eth_amount =
                oracle.convert(amount, Asset::ETH).await.context("failed to convert USD to ETH")?;
            Ok(eth_amount.value)
        }
        Asset::ZKC => bail!("ZKC is not valid for price fields. Use USD or ETH."),
    }
}

/// Convert a collateral Amount to ZKC.
///
/// Returns the value directly if the amount is already in ZKC.
/// Converts from USD to ZKC using the price oracle if needed.
/// Returns an error if:
/// - The amount is in USD but no price oracle is provided
/// - The amount is in an invalid asset (ETH not allowed for collateral)
async fn convert_amount_to_zkc(
    amount: &Amount,
    oracle: Option<&PriceOracleManager>,
) -> Result<U256> {
    match amount.asset {
        Asset::ZKC => Ok(amount.value),
        Asset::USD => {
            let oracle = oracle.context(
                "Price oracle required to convert USD collateral to ZKC. \
                 Configure a price oracle or specify collateral in ZKC.",
            )?;
            let zkc_amount =
                oracle.convert(amount, Asset::ZKC).await.context("failed to convert USD to ZKC")?;
            Ok(zkc_amount.value)
        }
        Asset::ETH => bail!("ETH is not valid for collateral. Use USD or ZKC."),
    }
}

pub(crate) const DEFAULT_TIMEOUT: u32 = 600;
pub(crate) const DEFAULT_RAMP_UP_PERIOD: u32 = 60;
/// Default min price when not set by params, config, or market (wei).
pub(crate) const DEFAULT_MIN_PRICE: U256 = U256::ZERO;
/// Default max price per cycle when not set by params, config, or market
/// (100 Kwei/cycle in wei to match 100 Gwei/Mcycle ~99th percentile of market as of 2026-02-11).
pub(crate) fn default_max_price_per_cycle() -> U256 {
    U256::from(100) * Unit::KWEI.wei_const()
}

/// Resolves min price (total) with priority: params > config > market > default (default is per-cycle × cycle_count).
pub(crate) fn resolve_min_price(
    params_min: Option<U256>,
    config_min_per_cycle: Option<U256>,
    cycle_count: Option<u64>,
    market_min: Option<U256>,
) -> U256 {
    params_min
        .or_else(|| cycle_count.and_then(|c| config_min_per_cycle.map(|p| p * U256::from(c))))
        .or(market_min)
        .unwrap_or(DEFAULT_MIN_PRICE)
}

/// Resolves max price (total) with priority: params > config > market > default (default is per-cycle × cycle_count).
pub(crate) fn resolve_max_price(
    params_max: Option<U256>,
    config_max: Option<U256>,
    market_max: Option<U256>,
    cycle_count: Option<u64>,
) -> U256 {
    params_max.or(config_max).or(market_max).unwrap_or_else(|| {
        // Use at least 1 so zero-cycle offers get a positive default max (fixed costs).
        let n = if let Some(count) = cycle_count {
            count.max(1)
        } else {
            tracing::warn!("No cycle count provided using static fallback, defaulting to 1");
            1
        };
        default_max_price_per_cycle() * U256::from(n)
    })
}

struct CollateralRecommendation {
    default: U256,
    large: U256,
    xl: U256,
}

impl CollateralRecommendation {
    fn new(default: U256, large: U256, xl: U256) -> Self {
        Self { default, large, xl }
    }

    /// Determine the recommended minimum collateral based on secondary performance and current collateral.
    ///
    /// Returns `Some(recommended_amount)` if the current collateral is too low, `None` otherwise.
    fn recommend_collateral(
        &self,
        secondary_performance: f64,
        lock_collateral: U256,
    ) -> anyhow::Result<Option<U256>> {
        let recommended = if secondary_performance < LARGE_REQUESTOR_LIST_THRESHOLD_KHZ
            && lock_collateral < self.default
        {
            Some(self.default)
        } else if (LARGE_REQUESTOR_LIST_THRESHOLD_KHZ..XL_REQUESTOR_LIST_THRESHOLD_KHZ)
            .contains(&secondary_performance)
            && lock_collateral < self.large
        {
            Some(self.large)
        } else if secondary_performance >= XL_REQUESTOR_LIST_THRESHOLD_KHZ
            && lock_collateral < self.xl
        {
            Some(self.xl)
        } else {
            None
        };
        Ok(recommended)
    }
}
/// Check if primary performance exceeds threshold and log a warning with recommended lock timeout.
///
/// Returns `true` if a warning was logged.
fn check_primary_performance_warning(cycle_count: u64, primary_performance: f64) -> bool {
    if primary_performance > XL_REQUESTOR_LIST_THRESHOLD_KHZ {
        let recommended_lock_timeout =
            cycle_count.div_ceil(XL_REQUESTOR_LIST_THRESHOLD_KHZ as u64) as u32;
        tracing::warn!(
            "Warning: your request requires a proving Khz of {primary_performance} to be \
             fulfilled before the lock timeout. This limits the number of provers in the \
             network that will be able to fulfill your order. Consider setting a longer \
             lock timeout of at least {recommended_lock_timeout} seconds."
        );
        true
    } else {
        false
    }
}

/// Check if secondary performance exceeds threshold and log a warning with recommended timeout.
///
/// Returns `true` if a warning was logged.
fn check_secondary_performance_warning(
    cycle_count: u64,
    secondary_performance: f64,
    lock_timeout: u32,
) -> bool {
    if secondary_performance > XL_REQUESTOR_LIST_THRESHOLD_KHZ {
        // Secondary prover has (timeout - lockTimeout) time available
        // Minimum time needed for secondary window: cycle_count / XL_threshold
        let min_secondary_window =
            cycle_count.div_ceil(XL_REQUESTOR_LIST_THRESHOLD_KHZ as u64) as u32;
        // Recommended total timeout = lockTimeout + minimum secondary window
        let recommended_timeout = lock_timeout.saturating_add(min_secondary_window);
        tracing::warn!(
            "Warning: your request requires a proving Khz of {secondary_performance} to be \
             fulfilled before the timeout. This limits the number of provers in the network \
             that will be able to fulfill your order. Consider setting a longer timeout of \
             at least {recommended_timeout} seconds."
        );
        true
    } else {
        false
    }
}

/// Configuration for the [OfferLayer].
///
/// Defines the default pricing parameters, timeouts, gas estimates, and other
/// settings used when constructing offers for proof requests.
#[non_exhaustive]
#[derive(Clone, Builder)]
pub struct OfferLayerConfig {
    /// Parameterization mode.
    ///
    /// Defines the offering parameters for the request. The default is
    /// [ParameterizationMode::fulfillment()], which is a conservative mode that ensures
    /// more provers can fulfill the request.
    ///
    /// # Example
    /// ```rust
    /// # use boundless_market::request_builder::{OfferLayerConfig, ParameterizationMode};
    ///
    /// OfferLayerConfig::builder().parameterization_mode(ParameterizationMode::fulfillment());
    /// ```
    #[builder(setter(into), default = "Some(ParameterizationMode::fulfillment())")]
    pub parameterization_mode: Option<ParameterizationMode>,

    /// Minimum price per RISC Zero execution cycle.
    ///
    /// Supports USD (e.g., `"0.00001 USD"`) or ETH (e.g., `"0.00000001 ETH"`).
    /// If USD is specified, it will be converted to ETH at runtime via the price oracle.
    #[builder(setter(into, strip_option), default)]
    pub min_price_per_cycle: Option<Amount>,

    /// Maximum price per RISC Zero execution cycle.
    ///
    /// Supports USD (e.g., `"0.00001 USD"`) or ETH (e.g., `"0.00000001 ETH"`).
    /// If USD is specified, it will be converted to ETH at runtime via the price oracle.
    #[builder(setter(into, strip_option), default)]
    pub max_price_per_cycle: Option<Amount>,

    /// Time in seconds to delay the start of bidding after request creation.
    #[builder(setter(strip_option), default)]
    pub bidding_start_delay: Option<u64>,

    /// Duration in seconds for the price to ramp up from min to max.
    #[builder(setter(strip_option), default)]
    pub ramp_up_period: Option<u32>,

    /// Time in seconds that a prover has to fulfill a locked request.
    #[builder(setter(strip_option), default)]
    pub lock_timeout: Option<u32>,

    /// Maximum time in seconds that a request can remain active.
    #[builder(setter(strip_option), default)]
    pub timeout: Option<u32>,

    /// Lock collateral amount.
    ///
    /// Supports USD (e.g., `"10 USD"`) or ZKC (e.g., `"20 ZKC"`).
    /// If USD is specified, it will be converted to ZKC at runtime via the price oracle.
    #[builder(setter(strip_option, into), default)]
    pub lock_collateral: Option<Amount>,

    /// Estimated gas used when locking a request.
    #[builder(default = "200_000")]
    pub lock_gas_estimate: u64,

    /// Estimated gas used when fulfilling a request (base cost, excluding journal calldata).
    #[builder(default = "400_000")]
    pub fulfill_gas_estimate: u64,

    /// Gas per byte of journal data submitted on-chain during fulfillment.
    ///
    /// Applied only for predicates that require journal data (DigestMatch, PrefixMatch).
    /// ClaimDigestMatch predicates do not submit journal data and skip this cost.
    #[builder(default = "26")]
    pub fulfill_journal_gas_per_byte: u64,

    /// Estimated gas used for Groth16 verification.
    #[builder(default = "250_000")]
    pub groth16_verify_gas_estimate: u64,

    /// Estimated gas used for ERC-1271 signature verification.
    #[builder(default = "100_000")]
    pub smart_contract_sig_verify_gas_estimate: u64,

    /// Supported proof types and their corresponding selectors.
    #[builder(setter(into), default)]
    pub supported_selectors: SupportedSelectors,
}

#[non_exhaustive]
/// A layer responsible for configuring the offer part of a proof request.
///
/// This layer uses an Ethereum provider to estimate gas costs and sets appropriate
/// pricing parameters for the proof request. It combines cycle count estimates with
/// gas price information to determine minimum and maximum prices for the request.
///
/// If a price provider is configured, it will be used to fetch market prices when
/// `OfferParams` doesn't explicitly set min_price or max_price.
pub struct OfferLayer<P> {
    /// The Ethereum provider used for gas price estimation.
    pub provider: P,

    /// Configuration for offer generation.
    pub config: OfferLayerConfig,

    /// Optional price provider for fetching market-based prices.
    /// If set, will be used when `OfferParams` doesn't specify prices.
    pub price_provider: Option<PriceProviderArc>,

    /// Optional price oracle manager for USD conversions.
    /// Required if `OfferLayerConfig` contains USD-denominated amounts.
    pub price_oracle_manager: Option<Arc<PriceOracleManager>>,
}

impl<P: Clone> Clone for OfferLayer<P> {
    fn clone(&self) -> Self {
        Self {
            provider: self.provider.clone(),
            config: self.config.clone(),
            price_provider: self.price_provider.clone(),
            price_oracle_manager: self.price_oracle_manager.clone(),
        }
    }
}

impl OfferLayerConfig {
    /// Creates a new builder for constructing an [OfferLayerConfig].
    ///
    /// This provides a way to customize pricing parameters, timeouts, and other
    /// offer-related settings used when generating proof requests.
    pub fn builder() -> OfferLayerConfigBuilder {
        Default::default()
    }
}

impl Default for OfferLayerConfig {
    fn default() -> Self {
        Self::builder().build().expect("implementation error in Default for OfferLayerConfig")
    }
}

impl<P: Clone> From<P> for OfferLayer<P> {
    fn from(provider: P) -> Self {
        OfferLayer {
            provider,
            config: Default::default(),
            price_provider: None,
            price_oracle_manager: None,
        }
    }
}

#[non_exhaustive]
#[derive(Clone, Debug, Default, Builder, Args)]
/// A partial [Offer], with all the fields as optional. Used in the [OfferLayer] to override
/// defaults set in the [OfferLayerConfig].
pub struct OfferParams {
    /// Minimum price willing to pay for the proof.
    /// Supports USD (e.g., `"0.00001 USD"`) or ETH (e.g., `"0.00000001 ETH"`).
    #[clap(long)]
    #[builder(setter(strip_option, into), default)]
    pub min_price: Option<Amount>,

    /// Maximum price willing to pay for the proof.
    /// Supports USD (e.g., `"0.001 USD"`) or ETH (e.g., `"0.00000001 ETH"`).
    #[clap(long)]
    #[builder(setter(strip_option, into), default)]
    pub max_price: Option<Amount>,

    /// Timestamp when bidding will start for this request.
    #[clap(long)]
    #[builder(setter(strip_option), default)]
    pub bidding_start: Option<u64>,

    /// Duration in seconds for the price to ramp up from min to max.
    #[clap(long)]
    #[builder(setter(strip_option), default)]
    pub ramp_up_period: Option<u32>,

    /// Time in seconds that a prover has to fulfill a locked request.
    #[clap(long)]
    #[builder(setter(strip_option), default)]
    pub lock_timeout: Option<u32>,

    /// Maximum time in seconds that a request can remain active.
    #[clap(long)]
    #[builder(setter(strip_option), default)]
    pub timeout: Option<u32>,

    /// Amount of the stake token that the prover must stake when locking a request.
    /// Supports USD (e.g., `"10 USD"`) or ZKC (e.g., `"20 ZKC"`).
    #[clap(long)]
    #[builder(setter(strip_option, into), default)]
    pub lock_collateral: Option<Amount>,
}

impl From<OfferParamsBuilder> for OfferParams {
    fn from(value: OfferParamsBuilder) -> Self {
        // Builder should be infallible.
        value.build().expect("implementation error in OfferParams")
    }
}

// Allows for a nicer builder pattern in RequestParams.
impl From<&mut OfferParamsBuilder> for OfferParams {
    fn from(value: &mut OfferParamsBuilder) -> Self {
        value.clone().into()
    }
}

impl OfferParams {
    /// Creates a new builder for constructing [OfferParams].
    ///
    /// Use this to set specific pricing parameters, timeouts, or other offer details
    /// that will override the defaults from [OfferLayerConfig].
    pub fn builder() -> OfferParamsBuilder {
        Default::default()
    }

    /// Create OfferParams from an on-chain Offer struct.
    ///
    /// On-chain offers always have ETH prices and ZKC collateral, so amounts
    /// are tagged with those assets.
    pub fn from_offer(offer: Offer) -> Self {
        Self {
            timeout: Some(offer.timeout),
            min_price: Some(Amount::new(offer.minPrice, Asset::ETH)),
            max_price: Some(Amount::new(offer.maxPrice, Asset::ETH)),
            lock_collateral: Some(Amount::new(offer.lockCollateral, Asset::ZKC)),
            lock_timeout: Some(offer.lockTimeout),
            bidding_start: Some(offer.rampUpStart),
            ramp_up_period: Some(offer.rampUpPeriod),
        }
    }

    /// Convert to an on-chain Offer struct.
    ///
    /// Price fields must be in ETH and collateral must be in ZKC.
    /// If any amounts are in USD, they will be converted using the provided price oracle.
    /// Returns an error if USD amounts are present but no oracle is provided.
    pub async fn into_offer(
        self,
        oracle: Option<&PriceOracleManager>,
    ) -> Result<Offer, anyhow::Error> {
        let min_price = self.min_price.ok_or(MissingFieldError::new("min_price"))?;
        let max_price = self.max_price.ok_or(MissingFieldError::new("max_price"))?;
        let lock_collateral =
            self.lock_collateral.ok_or(MissingFieldError::new("lock_collateral"))?;

        Ok(Offer {
            timeout: self.timeout.ok_or(MissingFieldError::new("timeout"))?,
            minPrice: convert_amount_to_eth(&min_price, oracle).await?,
            maxPrice: convert_amount_to_eth(&max_price, oracle).await?,
            lockCollateral: convert_amount_to_zkc(&lock_collateral, oracle).await?,
            lockTimeout: self.lock_timeout.ok_or(MissingFieldError::new("lock_timeout"))?,
            rampUpStart: self.bidding_start.ok_or(MissingFieldError::new("bidding_start"))?,
            rampUpPeriod: self.ramp_up_period.ok_or(MissingFieldError::new("ramp_up_period"))?,
        })
    }
}

impl<P> OfferLayer<P>
where
    P: Provider<Ethereum> + 'static + Clone,
{
    /// Creates a new [OfferLayer] with the given provider and configuration.
    ///
    /// The provider is used to fetch current gas prices for estimating transaction costs,
    /// which are factored into the offer pricing.
    pub fn new(provider: P, config: OfferLayerConfig) -> Self {
        Self { provider, config, price_provider: None, price_oracle_manager: None }
    }

    /// Set the price provider for the [OfferLayer].
    ///
    /// The price provider will be used to fetch market prices when `OfferParams` doesn't
    /// explicitly set min_price or max_price.
    ///
    /// # Parameters
    ///
    /// * `price_provider`: The price provider to use.
    ///
    /// # Returns
    ///
    /// A new [OfferLayer] with the price provider set.
    pub fn with_price_provider(self, price_provider: Option<PriceProviderArc>) -> Self {
        Self { price_provider, ..self }
    }

    /// Set the price oracle manager for USD conversions.
    ///
    /// The price oracle manager is required if the `OfferLayerConfig` contains
    /// USD-denominated amounts for pricing or collateral fields.
    ///
    /// # Parameters
    ///
    /// * `price_oracle_manager`: The price oracle manager to use.
    ///
    /// # Returns
    ///
    /// A new [OfferLayer] with the price oracle manager set.
    pub fn with_price_oracle_manager(
        self,
        price_oracle_manager: Option<Arc<PriceOracleManager>>,
    ) -> Self {
        Self { price_oracle_manager, ..self }
    }

    /// Convert a price Amount to ETH (wei).
    ///
    /// Returns `None` if the input is `None`.
    /// Returns the value directly if the amount is already in ETH.
    /// Converts from USD to ETH using the price oracle if needed.
    /// Returns an error if:
    /// - The amount is in USD but no price oracle is configured
    /// - The amount is in an invalid asset (ZKC not allowed for prices)
    async fn convert_price_to_eth(&self, amount: Option<&Amount>) -> Result<Option<U256>> {
        let Some(amount) = amount else { return Ok(None) };
        convert_amount_to_eth(amount, self.price_oracle_manager.as_deref()).await.map(Some)
    }

    /// Convert a collateral Amount to ZKC.
    ///
    /// Returns `None` if the input is `None`.
    /// Returns the value directly if the amount is already in ZKC.
    /// Converts from USD to ZKC using the price oracle if needed.
    /// Returns an error if:
    /// - The amount is in USD but no price oracle is configured
    /// - The amount is in an invalid asset (ETH not allowed for collateral)
    async fn convert_collateral_to_zkc(&self, amount: Option<&Amount>) -> Result<Option<U256>> {
        let Some(amount) = amount else { return Ok(None) };
        convert_amount_to_zkc(amount, self.price_oracle_manager.as_deref()).await.map(Some)
    }

    /// Estimates the maximum gas usage for a proof request.
    ///
    /// This calculates the upper bound of gas usage based on the request's requirements,
    /// configuration settings, and request ID characteristics.
    ///
    /// The estimate includes gas for locking, fulfilling, signature verification,
    /// callback execution, and proof verification.
    pub fn estimate_gas_usage_upper_bound(
        &self,
        requirements: &Requirements,
        request_id: &RequestId,
        journal_bytes: Option<usize>,
    ) -> anyhow::Result<u64> {
        let mut gas_usage_estimate =
            self.config.lock_gas_estimate + self.config.fulfill_gas_estimate;

        // Add gas for journal calldata when the predicate requires journal submission.
        if requirements.predicate.predicateType != PredicateType::ClaimDigestMatch {
            let len = match journal_bytes {
                Some(len) => len,
                None => {
                    const MAX_JOURNAL_BYTES: usize = max_journal_bytes();
                    tracing::warn!(
                        "Journal size unknown; using default estimate of {MAX_JOURNAL_BYTES} bytes \
                         for gas calculation"
                    );
                    MAX_JOURNAL_BYTES
                }
            };
            gas_usage_estimate += len as u64 * self.config.fulfill_journal_gas_per_byte;
        }

        if request_id.smart_contract_signed {
            gas_usage_estimate += self.config.smart_contract_sig_verify_gas_estimate;
        }
        if let Some(callback) = requirements.callback.as_option() {
            gas_usage_estimate +=
                u64::try_from(callback.gasLimit).context("callback gas limit too large for u64")?;
        }

        let proof_type = self
            .config
            .supported_selectors
            .proof_type(requirements.selector)
            .context("cannot estimate gas usage for request with unsupported selector")?;
        if let ProofType::Groth16 = proof_type {
            gas_usage_estimate += self.config.groth16_verify_gas_estimate;
        };
        Ok(gas_usage_estimate)
    }

    /// Estimates the maximum gas cost for a proof request.
    ///
    /// This calculates the cost in wei based on the estimated gas usage and
    /// the provided gas price.
    ///
    /// The result is used to determine appropriate pricing parameters for
    /// the proof request offer.
    pub fn estimate_gas_cost_upper_bound(
        &self,
        requirements: &Requirements,
        request_id: &RequestId,
        gas_price: u128,
        journal_bytes: Option<usize>,
    ) -> anyhow::Result<U256> {
        let gas_usage_estimate =
            self.estimate_gas_usage_upper_bound(requirements, request_id, journal_bytes)?;

        let gas_cost_estimate = gas_price * (gas_usage_estimate as u128);
        Ok(U256::from(gas_cost_estimate))
    }

    /// Computes max price as cycle-based price plus 2x current gas cost estimate.
    ///
    /// Fetches gas price from the provider, estimates gas cost for the request,
    /// and returns the sum with `max_price_cycle`.
    pub async fn max_price_with_gas(
        &self,
        requirements: &Requirements,
        request_id: &RequestId,
        max_price: U256,
        journal_bytes: Option<usize>,
    ) -> anyhow::Result<U256> {
        // Use high priority mode to give a buffer for gas price fluctuations.
        let gas_price: u128 = PriorityMode::High
            .estimate_max_fee_per_gas(&self.provider)
            .await
            .map_err(|err| anyhow::anyhow!("Failed to estimate gas price: {err:?}"))?;
        let gas_cost_estimate =
            self.estimate_gas_cost_upper_bound(requirements, request_id, gas_price, journal_bytes)?;

        // Use 2x the gas cost estimate to account for gas price fluctuations.
        let gas_cost_estimate = U256::from(2) * gas_cost_estimate;
        let adjusted_max_price = max_price + gas_cost_estimate;
        tracing::debug!(
            "Setting a max price of {} ether: {} max_price + {} gas_cost_estimate [gas price: {} gwei]",
            format_units(adjusted_max_price, "ether")?,
            format_units(max_price, "ether")?,
            format_units(gas_cost_estimate, "ether")?,
            format_units(U256::from(gas_price), "gwei")?,
        );
        Ok(adjusted_max_price)
    }
}

impl<P> Layer<(&Requirements, &RequestId, Option<u64>, Option<usize>, &OfferParams)>
    for OfferLayer<P>
where
    P: Provider<Ethereum> + 'static + Clone,
{
    type Output = Offer;
    type Error = anyhow::Error;

    async fn process(
        &self,
        (requirements, request_id, cycle_count, journal_bytes, params): (
            &Requirements,
            &RequestId,
            Option<u64>,
            Option<usize>,
            &OfferParams,
        ),
    ) -> Result<Self::Output, Self::Error> {
        // Convert config USD amounts to native tokens if needed
        let min_price_per_cycle_wei =
            self.convert_price_to_eth(self.config.min_price_per_cycle.as_ref()).await?;
        let max_price_per_cycle_wei =
            self.convert_price_to_eth(self.config.max_price_per_cycle.as_ref()).await?;
        let lock_collateral_zkc =
            self.convert_collateral_to_zkc(self.config.lock_collateral.as_ref()).await?;

        // Convert OfferParams USD amounts to native tokens if needed
        let params_min_price = self.convert_price_to_eth(params.min_price.as_ref()).await?;
        let params_max_price = self.convert_price_to_eth(params.max_price.as_ref()).await?;
        let params_lock_collateral =
            self.convert_collateral_to_zkc(params.lock_collateral.as_ref()).await?;

        // Try to use market prices from price provider if prices aren't set in params or config
        let (market_min_price, market_max_price) = if (params_min_price.is_none()
            && min_price_per_cycle_wei.is_none())
            || (params_max_price.is_none() && max_price_per_cycle_wei.is_none())
        {
            if let Some(ref price_provider) = self.price_provider {
                if let Some(cycle_count) = cycle_count {
                    match price_provider.price_percentiles().await {
                        Ok(percentiles) => {
                            let min = U256::ZERO;
                            let max = percentiles.p99.min(percentiles.p50 * U256::from(2))
                                * U256::from(cycle_count);
                            tracing::debug!(
                                "Using market prices from price provider: min={}, max={} (for {} cycles)",
                                format_units(min, "ether")?,
                                format_units(max, "ether")?,
                                cycle_count
                            );
                            (Some(min), Some(max))
                        }
                        Err(e) => {
                            tracing::warn!(
                                "Failed to fetch market prices from price provider: {}. Falling back to config-based pricing.",
                                e
                            );
                            (None, None)
                        }
                    }
                } else {
                    tracing::warn!("No cycle count provided, falling back to default pricing");
                    (None, None)
                }
            } else {
                (None, None)
            }
        } else {
            (None, None)
        };

        // Priority: params > config > market > static default
        let min_price = resolve_min_price(
            params_min_price,
            min_price_per_cycle_wei,
            cycle_count,
            market_min_price,
        );

        let config_max_value = if params_max_price.is_none() {
            let c = cycle_count.context("cycle count required to set max price in OfferLayer")?;
            if let Some(per_cycle) = max_price_per_cycle_wei {
                let max_price = per_cycle * U256::from(c);
                Some(max_price)
            } else {
                None
            }
        } else {
            None
        };

        let max_price =
            resolve_max_price(params_max_price, config_max_value, market_max_price, cycle_count);
        let max_price = if params_max_price.is_none() {
            self.max_price_with_gas(requirements, request_id, max_price, journal_bytes).await?
        } else {
            max_price
        };

        // Priority: config > recommended (from parameterization_mode) > default
        let (recommended_ramp_up_start, recommended_ramp_up_period, recommended_lock_timeout) =
            match self.config.parameterization_mode {
                Some(m) => (
                    Some(m.recommended_ramp_up_start(cycle_count)),
                    Some(m.recommended_ramp_up_period(cycle_count)),
                    Some(m.recommended_timeout(cycle_count)),
                ),
                None => (None, None, None),
            };

        let ramp_up_start = self
            .config
            .bidding_start_delay
            .map(|d| now_timestamp() + d)
            .or(recommended_ramp_up_start)
            .unwrap_or_else(|| now_timestamp() + 15);

        let ramp_up_period = self
            .config
            .ramp_up_period
            .or(recommended_ramp_up_period)
            .unwrap_or(DEFAULT_RAMP_UP_PERIOD);

        let lock_timeout = self
            .config
            .lock_timeout
            .or(recommended_lock_timeout)
            .unwrap_or(DEFAULT_TIMEOUT + DEFAULT_RAMP_UP_PERIOD);

        let timeout = self
            .config
            .timeout
            .or(Some(lock_timeout * 2))
            .unwrap_or((DEFAULT_TIMEOUT + DEFAULT_RAMP_UP_PERIOD) * 2);

        if self.config.bidding_start_delay.is_none() && recommended_ramp_up_start.is_none() {
            tracing::warn!("Using default ramp up start: {}", ramp_up_start);
        }
        if self.config.ramp_up_period.is_none() && recommended_ramp_up_period.is_none() {
            tracing::warn!("Using default ramp up period: {}", ramp_up_period);
        }
        if self.config.lock_timeout.is_none() && recommended_lock_timeout.is_none() {
            tracing::warn!("Using default lock timeout: {}", lock_timeout);
        }
        if self.config.timeout.is_none() && recommended_lock_timeout.is_none() {
            tracing::warn!("Using default timeout: {}", timeout);
        }

        let chain_id = self.provider.get_chain_id().await?;
        let default_collaterals = default_lock_collateral(chain_id);
        let lock_collateral = lock_collateral_zkc.unwrap_or(default_collaterals.default);

        let offer = Offer {
            minPrice: min_price,
            maxPrice: max_price,
            rampUpStart: params.bidding_start.unwrap_or(ramp_up_start),
            rampUpPeriod: params.ramp_up_period.unwrap_or(ramp_up_period),
            lockTimeout: params.lock_timeout.unwrap_or(lock_timeout),
            timeout: params.timeout.unwrap_or(timeout),
            lockCollateral: params_lock_collateral.unwrap_or(lock_collateral),
        };

        if let Some(cycle_count) = cycle_count {
            let primary_performance = offer.required_khz_performance(cycle_count);
            let secondary_performance =
                offer.required_khz_performance_secondary_prover(cycle_count);

            check_primary_performance_warning(cycle_count, primary_performance);
            check_secondary_performance_warning(
                cycle_count,
                secondary_performance,
                offer.lockTimeout,
            );

            // Check if the collateral requirement is low and raise a warning if it is.
            if let Some(collateral) = default_collaterals
                .recommend_collateral(secondary_performance, offer.lockCollateral)?
            {
                tracing::warn!(
                    "Warning: the collateral requirement of your request is low. This means the \
                     incentives for secondary provers to fulfill the order if the primary prover \
                     is slashed may be too low. It is recommended to set the lock collateral to at least {} ZKC.",
                    format_units(collateral, "ether")?
                );
            }
        }

        Ok(offer)
    }
}

/// Returns the default lock collateral for the given chain ID.
fn default_lock_collateral(chain_id: u64) -> CollateralRecommendation {
    match chain_id {
        8453 => CollateralRecommendation::new(
            U256::from(20) * Unit::ETHER.wei_const(),
            U256::from(50) * Unit::ETHER.wei_const(),
            U256::from(100) * Unit::ETHER.wei_const(),
        ), // Base mainnet - 20 ZKC
        84532 => CollateralRecommendation::new(
            U256::from(5) * Unit::ETHER.wei_const(),
            U256::from(10) * Unit::ETHER.wei_const(),
            U256::from(20) * Unit::ETHER.wei_const(),
        ), // Base Sepolia - 5 ZKC
        11155111 => CollateralRecommendation::new(
            U256::from(5) * Unit::ETHER.wei_const(),
            U256::from(10) * Unit::ETHER.wei_const(),
            U256::from(20) * Unit::ETHER.wei_const(),
        ), // Sepolia - 5 ZKC
        // Default for local/unknown chains (e.g., Anvil) - use similar to testnet defaults
        _ => CollateralRecommendation::new(
            U256::from(5) * Unit::ETHER.wei_const(),
            U256::from(10) * Unit::ETHER.wei_const(),
            U256::from(20) * Unit::ETHER.wei_const(),
        ),
    }
}

impl<P> Adapt<OfferLayer<P>> for RequestParams
where
    P: Provider<Ethereum> + 'static + Clone,
{
    type Output = RequestParams;
    type Error = anyhow::Error;

    async fn process_with(self, layer: &OfferLayer<P>) -> Result<Self::Output, Self::Error> {
        tracing::trace!("Processing {self:?} with OfferLayer");

        let requirements: Requirements = self
            .requirements
            .clone()
            .try_into()
            .context("failed to construct requirements in OfferLayer")?;
        let request_id = self.require_request_id()?;

        let journal_bytes = self.journal.as_ref().map(|j| j.bytes.len());
        let offer = layer
            .process((&requirements, request_id, self.cycles, journal_bytes, &self.offer))
            .await?;
        Ok(self.with_offer(OfferParams::from_offer(offer)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::primitives::utils::parse_ether;
    use tracing_test::traced_test;

    fn default_collaterals() -> CollateralRecommendation {
        CollateralRecommendation::new(
            parse_ether("20").unwrap(),
            parse_ether("50").unwrap(),
            parse_ether("100").unwrap(),
        )
    }

    mod performance_warnings {
        use super::*;

        #[test]
        fn primary_below_threshold() {
            assert!(!check_primary_performance_warning(1000, 5000.0));
        }

        #[test]
        #[traced_test]
        fn primary_above_threshold() {
            assert!(check_primary_performance_warning(20000, 15000.0));
            assert!(logs_contain("Warning: your request requires a proving Khz"));
            assert!(logs_contain("lock timeout"));
        }

        #[test]
        fn primary_at_threshold() {
            assert!(!check_primary_performance_warning(1000, XL_REQUESTOR_LIST_THRESHOLD_KHZ));
        }

        #[test]
        fn secondary_below_threshold() {
            assert!(!check_secondary_performance_warning(1000, 5000.0, 600));
        }

        #[test]
        #[traced_test]
        fn secondary_above_threshold() {
            assert!(check_secondary_performance_warning(20000, 15000.0, 600));
            assert!(logs_contain("Warning: your request requires a proving Khz"));
            assert!(logs_contain("timeout"));
        }

        #[test]
        fn secondary_at_threshold() {
            assert!(!check_secondary_performance_warning(
                1000,
                XL_REQUESTOR_LIST_THRESHOLD_KHZ,
                600
            ));
        }
    }

    mod collateral_recommendations {
        use super::*;

        #[test]
        fn low_performance_low_collateral() {
            let result = default_collaterals()
                .recommend_collateral(2000.0, parse_ether("10").unwrap())
                .unwrap();
            assert_eq!(result, Some(parse_ether("20").unwrap()));
        }

        #[test]
        fn low_performance_sufficient_collateral() {
            let result = default_collaterals()
                .recommend_collateral(2000.0, parse_ether("25").unwrap())
                .unwrap();
            assert_eq!(result, None);
        }

        #[test]
        fn medium_performance_low_collateral() {
            let result = default_collaterals()
                .recommend_collateral(5000.0, parse_ether("30").unwrap())
                .unwrap();
            assert_eq!(result, Some(parse_ether("50").unwrap()));
        }

        #[test]
        fn medium_performance_sufficient_collateral() {
            let result = default_collaterals()
                .recommend_collateral(5000.0, parse_ether("60").unwrap())
                .unwrap();
            assert_eq!(result, None);
        }

        #[test]
        fn high_performance_low_collateral() {
            let result = default_collaterals()
                .recommend_collateral(12000.0, parse_ether("80").unwrap())
                .unwrap();
            assert_eq!(result, Some(parse_ether("100").unwrap()));
        }

        #[test]
        fn high_performance_sufficient_collateral() {
            let result = default_collaterals()
                .recommend_collateral(12000.0, parse_ether("120").unwrap())
                .unwrap();
            assert_eq!(result, None);
        }

        #[test]
        fn at_large_threshold() {
            let result = default_collaterals()
                .recommend_collateral(
                    LARGE_REQUESTOR_LIST_THRESHOLD_KHZ,
                    parse_ether("10").unwrap(),
                )
                .unwrap();
            assert_eq!(result, Some(parse_ether("50").unwrap()));
        }

        #[test]
        fn at_xl_threshold() {
            let result = default_collaterals()
                .recommend_collateral(XL_REQUESTOR_LIST_THRESHOLD_KHZ, parse_ether("80").unwrap())
                .unwrap();
            assert_eq!(result, Some(parse_ether("100").unwrap()));
        }

        #[test]
        fn exact_threshold_low() {
            let result = default_collaterals()
                .recommend_collateral(2000.0, parse_ether("20").unwrap())
                .unwrap();
            assert_eq!(result, None);
        }

        #[test]
        fn exact_threshold_medium() {
            let result = default_collaterals()
                .recommend_collateral(5000.0, parse_ether("50").unwrap())
                .unwrap();
            assert_eq!(result, None);
        }

        #[test]
        fn exact_threshold_high() {
            let result = default_collaterals()
                .recommend_collateral(12000.0, parse_ether("100").unwrap())
                .unwrap();
            assert_eq!(result, None);
        }
    }

    mod price_priority {
        use super::*;

        fn u(n: u64) -> U256 {
            U256::from(n)
        }

        #[test]
        fn min_price_params_takes_priority() {
            assert_eq!(resolve_min_price(Some(u(1)), Some(u(2)), Some(10), Some(u(3))), u(1));
        }

        #[test]
        fn min_price_config_over_market_and_default() {
            assert_eq!(resolve_min_price(None, Some(u(5)), Some(10), Some(u(100))), u(50));
        }

        #[test]
        fn min_price_market_over_default() {
            assert_eq!(resolve_min_price(None, None, None, Some(u(42))), u(42));
        }

        #[test]
        fn min_price_default_when_all_none() {
            assert_eq!(
                resolve_min_price(None, None, None, None),
                DEFAULT_MIN_PRICE * U256::from(1)
            );
        }

        #[test]
        fn min_price_config_requires_cycle_count() {
            assert_eq!(resolve_min_price(None, Some(u(5)), None, Some(u(10))), u(10));
        }

        #[test]
        fn max_price_params_takes_priority() {
            assert_eq!(resolve_max_price(Some(u(1)), Some(u(2)), Some(u(3)), Some(10)), u(1));
        }

        #[test]
        fn max_price_config_over_market_and_default() {
            assert_eq!(resolve_max_price(None, Some(u(50)), Some(u(100)), Some(10)), u(50));
        }

        #[test]
        fn max_price_market_over_default() {
            assert_eq!(resolve_max_price(None, None, Some(u(42)), Some(10)), u(42));
        }

        #[test]
        fn max_price_default_when_all_none() {
            assert_eq!(
                resolve_max_price(None, None, None, Some(10)),
                default_max_price_per_cycle() * u(10)
            );
        }
    }

    mod offer_params_into_offer {
        use super::*;
        use crate::price_oracle::{
            cached_oracle::CachedPriceOracle, sources::static_source::StaticPriceSource,
            TradingPair,
        };

        /// Create a mock oracle manager with fixed prices for testing
        fn mock_oracle_manager(eth_usd_price: f64, zkc_usd_price: f64) -> PriceOracleManager {
            let eth_source = Arc::new(StaticPriceSource::new(TradingPair::EthUsd, eth_usd_price));
            let zkc_source = Arc::new(StaticPriceSource::new(TradingPair::ZkcUsd, zkc_usd_price));
            let eth_oracle = Arc::new(CachedPriceOracle::new(eth_source));
            let zkc_oracle = Arc::new(CachedPriceOracle::new(zkc_source));
            PriceOracleManager::new(eth_oracle, zkc_oracle, 60, 0)
        }

        #[tokio::test]
        async fn eth_and_zkc_passthrough_no_oracle() {
            // Native asset amounts should work without an oracle
            let params = OfferParams {
                min_price: Some(Amount::new(U256::from(1_000), Asset::ETH)),
                max_price: Some(Amount::new(U256::from(2_000), Asset::ETH)),
                lock_collateral: Some(Amount::new(U256::from(500), Asset::ZKC)),
                timeout: Some(300),
                lock_timeout: Some(120),
                bidding_start: Some(100),
                ramp_up_period: Some(60),
            };
            let offer = params.into_offer(None).await.unwrap();
            assert_eq!(offer.minPrice, U256::from(1_000));
            assert_eq!(offer.maxPrice, U256::from(2_000));
            assert_eq!(offer.lockCollateral, U256::from(500));
            assert_eq!(offer.timeout, 300);
            assert_eq!(offer.lockTimeout, 120);
            assert_eq!(offer.rampUpStart, 100);
            assert_eq!(offer.rampUpPeriod, 60);
        }

        #[tokio::test]
        async fn usd_prices_converted_with_oracle() {
            let manager = mock_oracle_manager(2000.0, 1.0);
            manager.refresh_all_rates().await;

            // $2000 USD = 1 ETH at $2000/ETH rate
            // $10 USD = 10 ZKC at $1/ZKC rate
            let params = OfferParams {
                min_price: Some(Amount::parse("1000 USD", None).unwrap()),
                max_price: Some(Amount::parse("2000 USD", None).unwrap()),
                lock_collateral: Some(Amount::parse("10 USD", None).unwrap()),
                timeout: Some(300),
                lock_timeout: Some(120),
                bidding_start: Some(100),
                ramp_up_period: Some(60),
            };
            let offer = params.into_offer(Some(&manager)).await.unwrap();

            // Verify converted amounts are non-zero and in expected ranges
            assert_eq!(offer.minPrice, parse_ether("0.5").unwrap());
            assert_eq!(offer.maxPrice, parse_ether("1").unwrap());
            assert_eq!(offer.lockCollateral, Amount::parse("10 ZKC", None).unwrap().value);
        }

        #[tokio::test]
        async fn usd_without_oracle_fails() {
            let params = OfferParams {
                min_price: Some(Amount::parse("100 USD", None).unwrap()),
                max_price: Some(Amount::parse("200 USD", None).unwrap()),
                lock_collateral: Some(Amount::parse("10 USD", None).unwrap()),
                timeout: Some(300),
                lock_timeout: Some(120),
                bidding_start: Some(100),
                ramp_up_period: Some(60),
            };
            let err = params.into_offer(None).await.unwrap_err();
            assert!(
                err.to_string().contains("Price oracle required"),
                "Expected error about price oracle, got: {}",
                err
            );
        }

        #[tokio::test]
        async fn zkc_in_price_field_fails() {
            let params = OfferParams {
                min_price: Some(Amount::new(U256::from(100), Asset::ZKC)),
                max_price: Some(Amount::new(U256::from(200), Asset::ETH)),
                lock_collateral: Some(Amount::new(U256::from(50), Asset::ZKC)),
                timeout: Some(300),
                lock_timeout: Some(120),
                bidding_start: Some(100),
                ramp_up_period: Some(60),
            };
            let err = params.into_offer(None).await.unwrap_err();
            assert!(
                err.to_string().contains("ZKC is not valid for price fields"),
                "Expected error about ZKC in price field, got: {}",
                err
            );
        }

        #[tokio::test]
        async fn eth_in_collateral_field_fails() {
            let params = OfferParams {
                min_price: Some(Amount::new(U256::from(100), Asset::ETH)),
                max_price: Some(Amount::new(U256::from(200), Asset::ETH)),
                lock_collateral: Some(Amount::new(U256::from(50), Asset::ETH)),
                timeout: Some(300),
                lock_timeout: Some(120),
                bidding_start: Some(100),
                ramp_up_period: Some(60),
            };
            let err = params.into_offer(None).await.unwrap_err();
            assert!(
                err.to_string().contains("ETH is not valid for collateral"),
                "Expected error about ETH in collateral field, got: {}",
                err
            );
        }
    }
}
