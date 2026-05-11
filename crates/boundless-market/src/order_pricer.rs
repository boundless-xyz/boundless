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

//! Backend-aware request-pricing trait used by the request builder's
//! pricing-check step.
//!
//! Each backend ships its own [`RequestPricer`] implementation; the SDK
//! constructs one via [`BackendRequestPricerFactory::make_pricer`] for the
//! environment (provider, market address, chain id) the request is being
//! built against.

use std::sync::Arc;

use alloy::{network::Ethereum, primitives::Address, providers::Provider};
use async_trait::async_trait;

use crate::{
    contracts::ProofRequest, price_oracle::PriceOracleManager, price_provider::PriceProviderArc,
    prover_utils::prover::ProverObj,
};

/// Environment available to backend request-pricer factories.
#[derive(Clone)]
#[non_exhaustive]
pub struct RequestPricingContext<P> {
    /// Prover shared with the request builder's preflight layer.
    pub prover: ProverObj,
    /// Alloy provider for market reads and gas estimation.
    pub provider: Arc<P>,
    /// Boundless market contract address.
    pub market_address: Address,
    /// Chain id of the target market.
    pub chain_id: u64,
    /// Optional external price provider configured on the offer layer.
    pub price_provider: Option<PriceProviderArc>,
    /// Optional price oracle manager configured on the offer layer.
    pub price_oracle: Option<Arc<PriceOracleManager>>,
}

/// Outcome of a single pricing check.
///
/// Numeric quantities use `work_units` semantics: each backend's pricer
/// interprets the value consistently with its own proving cost model.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub enum RequestPricingResult {
    /// The request was priced and is acceptable.
    Accept {
        /// Work units the order will consume.
        work_units: u64,
    },
    /// The request was priced and should be skipped.
    Skip {
        /// Human-readable reason describing why the request was skipped.
        reason: String,
    },
}

/// Backend-agnostic request pricer.
///
/// Used by [`crate::request_builder::StandardRequestBuilder`] to validate
/// that a constructed request is likely to be accepted by provers. Each
/// backend implements `price` against its own preflight + cost model.
#[async_trait]
pub trait RequestPricer: Send + Sync {
    /// Price a request and return the outcome.
    ///
    /// Implementations should not mutate global state; the request builder
    /// invokes this once per `build()` call and only inspects the result.
    async fn price(&self, request: &ProofRequest) -> anyhow::Result<RequestPricingResult>;
}

/// Companion trait that constructs a [`RequestPricer`] for a specific
/// environment.
///
/// Implementations are keyed off the alloy [`Provider`] type so the pricer
/// can capture an `Arc<P>` and the chain id for its own use.
pub trait BackendRequestPricerFactory<P>
where
    P: Provider<Ethereum> + 'static + Clone,
{
    /// Concrete request pricer the request builder will hold.
    type Pricer: RequestPricer + 'static;

    /// Build a pricer for the given environment.
    ///
    /// `prover` is the same instance the preflight layer holds, so backends
    /// that cache execution by content-address share that cache.
    fn make_pricer(ctx: RequestPricingContext<P>) -> Self::Pricer;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contracts::{Offer, Predicate, ProofRequest, RequestId, Requirements};
    use alloy::primitives::U256;

    /// Test pricer that records the request it received and returns a
    /// caller-supplied outcome. Validates the trait surface without any
    /// backend dependency.
    struct MockPricer {
        outcome: RequestPricingResult,
        seen: std::sync::Mutex<Option<ProofRequest>>,
    }

    #[async_trait]
    impl RequestPricer for MockPricer {
        async fn price(&self, request: &ProofRequest) -> anyhow::Result<RequestPricingResult> {
            *self.seen.lock().unwrap() = Some(request.clone());
            Ok(self.outcome.clone())
        }
    }

    fn sample_request() -> ProofRequest {
        ProofRequest::new(
            RequestId::new(Address::ZERO, 1),
            Requirements::new(Predicate::prefix_match(
                [0u8; 32],
                alloy::primitives::Bytes::default(),
            )),
            "http://example/program",
            crate::contracts::RequestInput {
                inputType: crate::contracts::RequestInputType::Inline,
                data: Default::default(),
            },
            Offer {
                minPrice: U256::ZERO,
                maxPrice: U256::from(1u64),
                rampUpStart: 0,
                rampUpPeriod: 1,
                lockTimeout: 100,
                timeout: 100,
                lockCollateral: U256::ZERO,
            },
        )
    }

    #[tokio::test]
    async fn mock_pricer_accept_outcome_round_trips() {
        let pricer = MockPricer {
            outcome: RequestPricingResult::Accept { work_units: 42 },
            seen: std::sync::Mutex::new(None),
        };
        let req = sample_request();
        let outcome = pricer.price(&req).await.unwrap();
        let RequestPricingResult::Accept { work_units } = outcome else {
            panic!("expected Accept outcome");
        };
        assert_eq!(work_units, 42);
        assert_eq!(pricer.seen.lock().unwrap().as_ref().unwrap().id, req.id);
    }

    #[tokio::test]
    async fn mock_pricer_skip_outcome_carries_reason() {
        let pricer = MockPricer {
            outcome: RequestPricingResult::Skip { reason: "test reason".into() },
            seen: std::sync::Mutex::new(None),
        };
        let outcome = pricer.price(&sample_request()).await.unwrap();
        let RequestPricingResult::Skip { reason } = outcome else {
            panic!("expected Skip outcome");
        };
        assert_eq!(reason, "test reason");
    }
}
