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

//! RISC Zero implementation of the SDK [`OrderPricer`] trait.
//!
//! Wraps the R0 cycle-based pricing pipeline (`requestor_order_preflight`)
//! behind the backend-agnostic [`OrderPricer`] surface, and registers
//! `RiscZeroBackend` as the [`Backend`] / [`BackendPricerFactory`].

use std::sync::Arc;

use alloy::{network::Ethereum, primitives::Address, providers::Provider};
use async_trait::async_trait;
use boundless_market::{
    contracts::ProofRequest, order_pricer::OrderPricingResult, price_oracle::PriceOracleManager,
    price_provider::PriceProviderArc, prover_utils::prover::ProverObj,
    prover_utils::requestor_order_preflight, Backend, BackendPricerFactory, OrderPricer,
};

use crate::risc_zero::RiscZeroBackend;

/// RISC Zero [`OrderPricer`].
///
/// Captures the alloy provider, market address, chain id, and pricing
/// dependencies the R0 pricing pipeline needs, plus a [`ProverObj`] shared
/// with the request builder's preflight layer so executions are cached.
#[derive(Clone)]
pub struct RiscZeroOrderPricer<P> {
    prover: ProverObj,
    provider: Arc<P>,
    market_address: Address,
    chain_id: u64,
    price_provider: Option<PriceProviderArc>,
    price_oracle: Option<Arc<PriceOracleManager>>,
}

impl<P> RiscZeroOrderPricer<P>
where
    P: Provider<Ethereum> + 'static + Clone,
{
    /// Construct a new R0 order pricer.
    pub fn new(
        prover: ProverObj,
        provider: Arc<P>,
        market_address: Address,
        chain_id: u64,
        price_provider: Option<PriceProviderArc>,
        price_oracle: Option<Arc<PriceOracleManager>>,
    ) -> Self {
        Self { prover, provider, market_address, chain_id, price_provider, price_oracle }
    }
}

#[async_trait]
impl<P> OrderPricer for RiscZeroOrderPricer<P>
where
    P: Provider<Ethereum> + 'static + Clone,
{
    async fn price(&self, request: &ProofRequest) -> anyhow::Result<OrderPricingResult> {
        let work_units = requestor_order_preflight(
            request.clone(),
            self.prover.clone(),
            self.provider.clone(),
            self.market_address,
            self.chain_id,
            self.price_provider.clone(),
            self.price_oracle.clone(),
        )
        .await?;
        Ok(match work_units {
            Some(work_units) => OrderPricingResult::Lock { work_units },
            None => OrderPricingResult::Skip {
                reason: "preflight rejected the order (see logs for details)".into(),
            },
        })
    }
}

impl Backend for RiscZeroBackend {}

impl<P> BackendPricerFactory<P> for RiscZeroBackend
where
    P: Provider<Ethereum> + 'static + Clone,
{
    type Pricer = RiscZeroOrderPricer<P>;

    fn make_pricer(
        prover: ProverObj,
        provider: Arc<P>,
        market_address: Address,
        chain_id: u64,
        price_provider: Option<PriceProviderArc>,
        price_oracle: Option<Arc<PriceOracleManager>>,
    ) -> Self::Pricer {
        RiscZeroOrderPricer::new(
            prover,
            provider,
            market_address,
            chain_id,
            price_provider,
            price_oracle,
        )
    }
}
