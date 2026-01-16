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
    contracts::{Offer, RequestId, Requirements},
    price_provider::PriceProviderArc,
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
use anyhow::{Context, Result};
use clap::Args;
use derive_builder::Builder;

pub(crate) const DEFAULT_TIMEOUT: u32 = 600;
pub(crate) const DEFAULT_RAMP_UP_PERIOD: u32 = 60;
pub(crate) const DEFAULT_MAX_TIMEOUT: u32 = 14400; // 4 hours

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
    /// Defines the offering parameters for the request based on the mode:
    /// - [ParameterizationMode::fulfillment] is a more conservative mode that ensures more provers can fulfill the request.
    /// - [ParameterizationMode::latency] is a more aggressive mode that allows for faster fulfillment at the cost of higher prices and lower fulfillment guarantees.
    ///
    /// The default is [ParameterizationMode::fulfillment()], which is the default fulfillment mode.
    ///
    /// # Example
    /// ```rust
    /// # use boundless_market::request_builder::{OfferLayerConfig, ParameterizationMode};
    /// use boundless_market::request_builder::ParameterizationMode;
    ///
    /// OfferLayerConfig::builder().parameterization_mode(ParameterizationMode::latency());
    /// OfferLayerConfig::builder().parameterization_mode(ParameterizationMode::fulfillment());
    /// ```
    #[builder(setter(into), default = "Some(ParameterizationMode::fulfillment())")]
    pub parameterization_mode: Option<ParameterizationMode>,

    /// Minimum price per RISC Zero execution cycle, in wei.
    #[builder(setter(into), default = "U256::ZERO")]
    pub min_price_per_cycle: U256,

    /// Maximum price per RISC Zero execution cycle, in wei.
    #[builder(setter(into), default = "U256::from(100) * Unit::MWEI.wei_const()")]
    pub max_price_per_cycle: U256,

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

    /// Lock collateral
    #[builder(setter(strip_option, into), default)]
    pub lock_collateral: Option<U256>,

    /// Estimated gas used when locking a request.
    #[builder(default = "200_000")]
    pub lock_gas_estimate: u64,

    /// Estimated gas used when fulfilling a request.
    #[builder(default = "750_000")]
    pub fulfill_gas_estimate: u64,

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
}

impl<P: Clone> Clone for OfferLayer<P> {
    fn clone(&self) -> Self {
        Self {
            provider: self.provider.clone(),
            config: self.config.clone(),
            price_provider: self.price_provider.clone(),
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
        OfferLayer { provider, config: Default::default(), price_provider: None }
    }
}

#[non_exhaustive]
#[derive(Clone, Debug, Default, Builder, Args)]
/// A partial [Offer], with all the fields as optional. Used in the [OfferLayer] to override
/// defaults set in the [OfferLayerConfig].
pub struct OfferParams {
    /// Minimum price willing to pay for the proof, in wei.
    #[clap(long)]
    #[builder(setter(strip_option, into), default)]
    pub min_price: Option<U256>,

    /// Maximum price willing to pay for the proof, in wei.
    #[clap(long)]
    #[builder(setter(strip_option, into), default)]
    pub max_price: Option<U256>,

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
    #[clap(long)]
    #[builder(setter(strip_option, into), default)]
    pub lock_collateral: Option<U256>,
}

impl From<Offer> for OfferParams {
    fn from(value: Offer) -> Self {
        Self {
            timeout: Some(value.timeout),
            min_price: Some(value.minPrice),
            max_price: Some(value.maxPrice),
            lock_collateral: Some(value.lockCollateral),
            lock_timeout: Some(value.lockTimeout),
            bidding_start: Some(value.rampUpStart),
            ramp_up_period: Some(value.rampUpPeriod),
        }
    }
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

impl TryFrom<OfferParams> for Offer {
    type Error = MissingFieldError;

    fn try_from(value: OfferParams) -> Result<Self, Self::Error> {
        Ok(Self {
            timeout: value.timeout.ok_or(MissingFieldError::new("timeout"))?,
            minPrice: value.min_price.ok_or(MissingFieldError::new("min_price"))?,
            maxPrice: value.max_price.ok_or(MissingFieldError::new("max_price"))?,
            lockCollateral: value
                .lock_collateral
                .ok_or(MissingFieldError::new("lock_collateral"))?,
            lockTimeout: value.lock_timeout.ok_or(MissingFieldError::new("lock_timeout"))?,
            rampUpStart: value.bidding_start.ok_or(MissingFieldError::new("bidding_start"))?,
            rampUpPeriod: value.ramp_up_period.ok_or(MissingFieldError::new("ramp_up_period"))?,
        })
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
        Self { provider, config, price_provider: None }
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
    ) -> anyhow::Result<u64> {
        let mut gas_usage_estimate =
            self.config.lock_gas_estimate + self.config.fulfill_gas_estimate;
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
    ) -> anyhow::Result<U256> {
        let gas_usage_estimate = self.estimate_gas_usage_upper_bound(requirements, request_id)?;

        let gas_cost_estimate = gas_price * (gas_usage_estimate as u128);
        Ok(U256::from(gas_cost_estimate))
    }
}

impl<P> Layer<(&Requirements, &RequestId, Option<u64>, &OfferParams)> for OfferLayer<P>
where
    P: Provider<Ethereum> + 'static + Clone,
{
    type Output = Offer;
    type Error = anyhow::Error;

    async fn process(
        &self,
        (requirements, request_id, cycle_count, params): (
            &Requirements,
            &RequestId,
            Option<u64>,
            &OfferParams,
        ),
    ) -> Result<Self::Output, Self::Error> {
        // Try to use market prices from price provider if prices aren't set in params
        let (market_min_price, market_max_price) = if params.min_price.is_none()
            || params.max_price.is_none()
        {
            if let Some(ref price_provider) = self.price_provider {
                if let Some(cycle_count) = cycle_count {
                    match price_provider.price_percentiles().await {
                        Ok(percentiles) => {
                            let min = percentiles.p10 * U256::from(cycle_count);
                            let max = percentiles.p90 * U256::from(cycle_count);
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
                    (None, None)
                }
            } else {
                (None, None)
            }
        } else {
            (None, None)
        };

        let min_price = if params.min_price.is_none() {
            // Use market price if available, otherwise fall back to config
            if let Some(market_min) = market_min_price {
                market_min
            } else {
                match cycle_count {
                    Some(cycle_count) => self.config.min_price_per_cycle * U256::from(cycle_count),
                    None => {
                        if self.config.min_price_per_cycle != U256::ZERO {
                            return Err(anyhow::anyhow!(
                                "cycle count required to set min price in OfferLayer"
                            ));
                        }
                        U256::ZERO
                    }
                }
            }
        } else {
            params.min_price.unwrap()
        };

        let max_price = if params.max_price.is_none() {
            // Use market price if available, otherwise fall back to config + gas
            if let Some(market_max) = market_max_price {
                market_max
            } else {
                let cycle_count =
                    cycle_count.context("cycle count required to set max price in OfferLayer")?;
                let max_price_cycle = self.config.max_price_per_cycle * U256::from(cycle_count);

                let gas_price: u128 = self.provider.get_gas_price().await?;
                let gas_cost_estimate =
                    self.estimate_gas_cost_upper_bound(requirements, request_id, gas_price)?;

                // Add the gas price plus 10% to the max_price.
                let adjusted_gas_cost_estimate =
                    gas_cost_estimate + (gas_cost_estimate / U256::from(10));
                let max_price = max_price_cycle + adjusted_gas_cost_estimate;
                tracing::debug!(
                    "Setting a max price of {} ether: {} cycle_price + {} gas_cost_estimate (adjusted by 10%) [gas price: {} gwei]",
                    format_units(max_price, "ether")?,
                    format_units(max_price_cycle, "ether")?,
                    format_units(adjusted_gas_cost_estimate, "ether")?,
                    format_units(U256::from(gas_price), "gwei")?,
                );
                max_price
            }
        } else {
            params.max_price.unwrap()
        };

        let (lock_timeout, timeout, ramp_up_period, ramp_up_start) =
            if let Some(parameterization_mode) = self.config.parameterization_mode {
                let ramp_up_start = self
                    .config
                    .bidding_start_delay
                    .unwrap_or(parameterization_mode.recommended_ramp_up_start(cycle_count));
                let ramp_up_period = self
                    .config
                    .ramp_up_period
                    .unwrap_or(parameterization_mode.recommended_ramp_up_period(cycle_count));
                let recommended_timeout =
                    parameterization_mode.recommended_timeout(cycle_count) + ramp_up_period;
                let lock_timeout = self
                    .config
                    .lock_timeout
                    .unwrap_or(recommended_timeout.min(DEFAULT_MAX_TIMEOUT));
                let timeout = self.config.timeout.unwrap_or(
                    (recommended_timeout * 2 - ramp_up_period).min(DEFAULT_MAX_TIMEOUT * 2),
                );
                (lock_timeout, timeout, ramp_up_period, ramp_up_start)
            } else {
                let lock_timeout = DEFAULT_TIMEOUT + DEFAULT_RAMP_UP_PERIOD;
                let timeout =
                    ((DEFAULT_TIMEOUT + DEFAULT_RAMP_UP_PERIOD) * 2) - DEFAULT_RAMP_UP_PERIOD;
                let ramp_up_period = DEFAULT_RAMP_UP_PERIOD;
                let ramp_up_start = now_timestamp() + 15;
                tracing::warn!("Using default ramp up start: {}", ramp_up_start);
                tracing::warn!("Using default ramp up period: {}", ramp_up_period);
                tracing::warn!("Using default lock timeout: {}", lock_timeout);
                tracing::warn!("Using default timeout: {}", timeout);
                (lock_timeout, timeout, ramp_up_period, ramp_up_start)
            };

        let chain_id = self.provider.get_chain_id().await?;
        let default_collaterals = default_lock_collateral(chain_id);
        let lock_collateral = self.config.lock_collateral.unwrap_or(default_collaterals.default);

        let mut offer = Offer {
            minPrice: min_price,
            maxPrice: max_price,
            rampUpStart: params.bidding_start.unwrap_or(ramp_up_start),
            rampUpPeriod: params.ramp_up_period.unwrap_or(ramp_up_period),
            lockTimeout: params.lock_timeout.unwrap_or(lock_timeout),
            timeout: params.timeout.unwrap_or(timeout),
            lockCollateral: params.lock_collateral.unwrap_or(lock_collateral),
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
                     is slashed may be too low. Overriding your lock collateral to {} ZKC.",
                    format_units(collateral, "ether")?
                );
                offer.lockCollateral = collateral;
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
        _ => CollateralRecommendation::new(U256::ZERO, U256::ZERO, U256::ZERO), // No default lock collateral for other chains
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

        let offer = layer.process((&requirements, request_id, self.cycles, &self.offer)).await?;
        Ok(self.with_offer(offer))
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
}
