// Copyright 2025 RISC Zero, Inc.
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
    selector::{ProofType, SupportedSelectors},
    util::now_timestamp,
};
use alloy::{
    network::Ethereum,
    primitives::{
        utils::{format_units, Unit},
        U256,
    },
    providers::Provider,
};
use anyhow::{ensure, Context};
use clap::Args;
use derive_builder::Builder;

/// Configuration for the [OfferLayer].
///
/// Defines the default pricing parameters, timeouts, gas estimates, and other
/// settings used when constructing offers for proof requests.
#[non_exhaustive]
#[derive(Clone, Builder)]
pub struct OfferLayerConfig {
    /// Minimum price per RISC Zero execution cycle, in wei.
    #[builder(setter(into), default = "U256::ZERO")]
    pub min_price_per_cycle: U256,

    /// Maximum price per RISC Zero execution cycle, in wei.
    #[builder(setter(into), default = "U256::from(100) * Unit::MWEI.wei_const()")]
    pub max_price_per_cycle: U256,

    /// Time in seconds to delay the start of bidding after request creation.
    #[builder(default = "15")]
    pub bidding_start_delay: u64,

    /// Duration in seconds for the price to ramp up from min to max.
    #[builder(default = "60")]
    pub ramp_up_period: u32,

    /// Time in seconds that a prover has to fulfill a locked request.
    #[builder(default = "600")]
    pub lock_timeout: u32,

    /// Maximum time in seconds that a request can remain active.
    #[builder(default = "1200")]
    pub timeout: u32,

    /// Amount of the stake token that the prover must stake when locking a request.
    #[builder(setter(into), default = "U256::from(100) * Unit::PWEI.wei_const()")]
    pub lock_stake: U256,

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
#[derive(Clone)]
/// A layer responsible for configuring the offer part of a proof request.
///
/// This layer uses an Ethereum provider to estimate gas costs and sets appropriate
/// pricing parameters for the proof request. It combines cycle count estimates with
/// gas price information to determine minimum and maximum prices for the request.
pub struct OfferLayer<P> {
    /// The Ethereum provider used for gas price estimation.
    pub provider: P,

    /// Configuration for offer generation.
    pub config: OfferLayerConfig,
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
        OfferLayer { provider, config: Default::default() }
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
    pub lock_stake: Option<U256>,
}

impl From<Offer> for OfferParams {
    fn from(value: Offer) -> Self {
        Self {
            timeout: Some(value.timeout),
            min_price: Some(value.minPrice),
            max_price: Some(value.maxPrice),
            lock_stake: Some(value.lockStake),
            lock_timeout: Some(value.lockTimeout),
            bidding_start: Some(value.biddingStart),
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
            lockStake: value.lock_stake.ok_or(MissingFieldError::new("lock_stake"))?,
            lockTimeout: value.lock_timeout.ok_or(MissingFieldError::new("lock_timeout"))?,
            biddingStart: value.bidding_start.ok_or(MissingFieldError::new("bidding_start"))?,
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
        Self { provider, config }
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
        let min_price = if params.min_price.is_none() {
            match cycle_count {
                Some(cycle_count) => self.config.min_price_per_cycle * U256::from(cycle_count),
                None => {
                    ensure!(
                        self.config.min_price_per_cycle == U256::ZERO,
                        "cycle count required to set min price in OfferLayer"
                    );
                    U256::ZERO
                }
            }
        } else {
            params.min_price.unwrap()
        };

        let max_price = if params.max_price.is_none() {
            let cycle_count =
                cycle_count.context("cycle count required to set max price in OfferLayer")?;
            let max_price_cycle = self.config.max_price_per_cycle * U256::from(cycle_count);

            let gas_price: u128 = self.provider.get_gas_price().await?;
            let gas_cost_estimate =
                self.estimate_gas_cost_upper_bound(requirements, request_id, gas_price)?;

            // Add the gas price plus 10% to the max_price.
            let max_price =
                max_price_cycle + (gas_cost_estimate + (gas_cost_estimate / U256::from(10)));
            tracing::debug!(
                "Setting a max price of {} ether: {} cycle_price + {} gas_cost_estimate",
                format_units(max_price, "ether")?,
                format_units(max_price_cycle, "ether")?,
                format_units(gas_cost_estimate, "ether")?,
            );
            max_price
        } else {
            params.max_price.unwrap()
        };

        let bidding_start = params
            .bidding_start
            .unwrap_or_else(|| now_timestamp() + self.config.bidding_start_delay);

        Ok(Offer {
            minPrice: min_price,
            maxPrice: max_price,
            biddingStart: bidding_start,
            rampUpPeriod: params.ramp_up_period.unwrap_or(self.config.ramp_up_period),
            lockTimeout: params.lock_timeout.unwrap_or(self.config.lock_timeout),
            timeout: params.timeout.unwrap_or(self.config.timeout),
            lockStake: params.lock_stake.unwrap_or(self.config.lock_stake),
        })
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
