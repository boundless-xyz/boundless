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

use super::{Adapt, Layer, RequestParams};
use crate::{
    contracts::{Offer, RequestId, Requirements},
    selector::{ProofType, SupportedSelectors},
    util::{now_timestamp},
};
use alloy::{
    network::Ethereum,
    primitives::{
        utils::{format_units, Unit},
        U256,
    },
    providers::Provider,
};
use anyhow::Context;
use derive_builder::Builder;

#[non_exhaustive]
#[derive(Clone, Builder)]
pub struct OfferLayerConfig {
    #[builder(setter(into), default = "U256::ZERO")]
    pub min_price_per_mcycle: U256,
    #[builder(setter(into), default = "U256::from(100) * Unit::TWEI.wei_const()")]
    pub max_price_per_mcycle: U256,
    #[builder(default = "15")]
    pub bidding_start_delay: u64,
    #[builder(default = "120")]
    pub ramp_up_period: u32,
    #[builder(default = "600")]
    pub lock_timeout: u32,
    #[builder(default = "1200")]
    pub timeout: u32,
    #[builder(setter(into), default = "U256::from(100) * Unit::PWEI.wei_const()")]
    pub lock_stake: U256,
    #[builder(default = "200_000")]
    pub lock_gas_estimate: u64,
    #[builder(default = "750_000")]
    pub fulfill_gas_estimate: u64,
    #[builder(default = "250_000")]
    pub groth16_verify_gas_estimate: u64,
    #[builder(default = "100_000")]
    pub smart_contract_sig_verify_gas_estimate: u64,
    #[builder(setter(into), default)]
    pub supported_selectors: SupportedSelectors,
}

#[non_exhaustive]
#[derive(Clone)]
pub struct OfferLayer<P> {
    pub provider: P,
    pub config: OfferLayerConfig,
}

impl OfferLayerConfig {
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

impl<P> OfferLayer<P>
where
    P: Provider<Ethereum> + 'static + Clone,
{
    pub fn new(provider: P, config: OfferLayerConfig) -> Self {
        Self { provider, config }
    }

    fn estimate_gas_usage(
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

    fn estimate_gas_cost(
        &self,
        requirements: &Requirements,
        request_id: &RequestId,
        gas_price: u128,
    ) -> anyhow::Result<U256> {
        let gas_usage_estimate = self.estimate_gas_usage(requirements, request_id)?;

        let gas_cost_estimate = (gas_price + (gas_price / 10)) * (gas_usage_estimate as u128);
        Ok(U256::from(gas_cost_estimate))
    }
}

impl<P> Layer<(&Requirements, &RequestId, u64)> for OfferLayer<P>
where
    P: Provider<Ethereum> + 'static + Clone,
{
    type Output = Offer;
    type Error = anyhow::Error;

    async fn process(
        &self,
        (requirements, request_id, cycle_count): (&Requirements, &RequestId, u64),
    ) -> Result<Self::Output, Self::Error> {
        let mcycle_count = cycle_count >> 20;
        let min_price = self.config.min_price_per_mcycle * U256::from(mcycle_count);
        let max_price_mcycle = self.config.max_price_per_mcycle * U256::from(mcycle_count);

        let gas_price: u128 = self.provider.get_gas_price().await?;
        let gas_cost_estimate = self.estimate_gas_cost(requirements, request_id, gas_price)?;
        let max_price = max_price_mcycle + gas_cost_estimate;
        tracing::debug!(
            "Setting a max price of {} ether: {} mcycle_price + {} gas_cost_estimate",
            format_units(max_price, "ether")?,
            format_units(max_price_mcycle, "ether")?,
            format_units(gas_cost_estimate, "ether")?,
        );

        Ok(Offer {
            minPrice: min_price,
            maxPrice: max_price,
            biddingStart: now_timestamp() + self.config.bidding_start_delay,
            rampUpPeriod: self.config.ramp_up_period,
            lockTimeout: self.config.lock_timeout,
            timeout: self.config.timeout,
            lockStake: self.config.lock_stake,
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

        if self.offer.is_some() {
            return Ok(self);
        }

        let requirements = self.require_requirements()?;
        let request_id = self.require_request_id()?;
        let cycles = self.require_cycles()?;

        let offer = layer.process((requirements, request_id, cycles)).await?;
        Ok(self.with_offer(offer))
    }
}
