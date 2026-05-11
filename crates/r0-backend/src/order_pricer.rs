// Copyright 2026 Boundless Foundation, Inc.
//
// Use of this source code is governed by the Business Source License
// as found in the LICENSE-BSL file.

//! RISC Zero implementation of the SDK [`RequestPricer`] trait.
//!
//! Wraps the R0 cycle-based pricing pipeline (`requestor_order_preflight`)
//! behind the backend-agnostic [`RequestPricer`] surface, and registers
//! [`RiscZeroSdkBackend`] as the [`Backend`] / [`BackendRequestPricerFactory`].

use std::sync::Arc;

use alloy::{network::Ethereum, primitives::Address, providers::Provider};
use async_trait::async_trait;
use boundless_market::{
    contracts::ProofRequest, order_pricer::RequestPricingResult, price_oracle::PriceOracleManager,
    price_provider::PriceProviderArc, prover_utils::prover::ProverObj,
    prover_utils::requestor_order_preflight, Backend, BackendRequestPricerFactory, RequestPricer,
};

/// SDK marker for RISC Zero request-pricing behavior.
///
/// This is intentionally separate from [`crate::RiscZeroBackend`], which is a
/// stateful runtime [`boundless_market::BackendProvider`] implementation.
#[derive(Clone, Copy, Debug, Default)]
pub struct RiscZeroSdkBackend;

impl Backend for RiscZeroSdkBackend {}

/// RISC Zero [`RequestPricer`].
///
/// Captures the alloy provider, market address, chain id, and pricing
/// dependencies the R0 pricing pipeline needs, plus a [`ProverObj`] shared
/// with the request builder's preflight layer so executions are cached.
#[derive(Clone)]
pub struct RiscZeroRequestPricer<P> {
    prover: ProverObj,
    provider: Arc<P>,
    market_address: Address,
    chain_id: u64,
    price_provider: Option<PriceProviderArc>,
    price_oracle: Option<Arc<PriceOracleManager>>,
}

impl<P> RiscZeroRequestPricer<P>
where
    P: Provider<Ethereum> + 'static + Clone,
{
    /// Construct a new R0 request pricer.
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
impl<P> RequestPricer for RiscZeroRequestPricer<P>
where
    P: Provider<Ethereum> + 'static + Clone,
{
    async fn price(&self, request: &ProofRequest) -> anyhow::Result<RequestPricingResult> {
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
            Some(work_units) => RequestPricingResult::Lock { work_units },
            None => RequestPricingResult::Skip {
                reason: "preflight rejected the order (see logs for details)".into(),
            },
        })
    }
}

impl<P> BackendRequestPricerFactory<P> for RiscZeroSdkBackend
where
    P: Provider<Ethereum> + 'static + Clone,
{
    type Pricer = RiscZeroRequestPricer<P>;

    fn make_pricer(
        prover: ProverObj,
        provider: Arc<P>,
        market_address: Address,
        chain_id: u64,
        price_provider: Option<PriceProviderArc>,
        price_oracle: Option<Arc<PriceOracleManager>>,
    ) -> Self::Pricer {
        RiscZeroRequestPricer::new(
            prover,
            provider,
            market_address,
            chain_id,
            price_provider,
            price_oracle,
        )
    }
}
