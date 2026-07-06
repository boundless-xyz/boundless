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

use std::sync::Arc;

use alloy::primitives::{Address, Bytes, FixedBytes};
use anyhow::Context;
use async_trait::async_trait;
use moka::future::Cache;
use sha2::{Digest, Sha256};

use super::{OrderPricingError, OrderRequest};
use crate::contracts::RequestInputType;

/// Result of evaluating a request against a backend.
#[derive(Clone, Debug)]
pub enum RequestEvaluation {
    Success {
        evaluation_id: String,
        metrics: EvaluationMetrics,
        program_id: String,
        input_id: String,
        public_output: Vec<u8>,
    },
    LimitExceeded {
        limit: EvaluationLimits,
    },
    GuestFailed,
}

/// Backend-native work observed during request evaluation.
///
/// The unit is defined by the evaluator. For the RISC0 evaluator it is the cycle count.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub struct NativeWork {
    pub units: u64,
}

impl NativeWork {
    pub fn new(units: u64) -> Self {
        Self { units }
    }
}

/// Broker-comparable work units derived from backend-native evaluation output.
///
/// TODO(zkvm-abstraction): cross-backend normalization unit is currently RISC0 cycles 1:1.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub struct NormalizedWork {
    pub units: u64,
}

impl NormalizedWork {
    pub fn new(units: u64) -> Self {
        Self { units }
    }
}

/// Request evaluation metrics returned by a backend.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub struct EvaluationMetrics {
    pub native: NativeWork,
    pub normalized: NormalizedWork,
}

impl EvaluationMetrics {
    pub fn new(native: NativeWork, normalized: NormalizedWork) -> Self {
        Self { native, normalized }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PreflightErrorKind {
    LimitExceeded,
    GuestPanicked,
}

/// Value type for the preflight cache.
pub type PreflightCacheValue = RequestEvaluation;

/// Input type for preflight cache.
#[derive(Hash, Eq, PartialEq, Clone, Debug)]
pub enum InputCacheKey {
    /// URL-based input.
    Url(String),
    /// Hash-based input (for inline data).
    Hash([u8; 32]),
}

/// Key type for the preflight cache.
#[derive(Hash, Eq, PartialEq, Clone, Debug)]
pub struct PreflightCacheKey {
    /// The resolved program identity.
    pub program_id: String,
    /// The requested verifier selector.
    pub selector: FixedBytes<4>,
    /// The predicate data.
    pub predicate_data: Vec<u8>,
    /// The input cache key.
    pub input: InputCacheKey,
    /// The cycle limit the evaluation ran under.
    pub max_cycles: u64,
}

/// Cache for preflight results.
pub type PreflightCache = Arc<Cache<PreflightCacheKey, PreflightCacheValue>>;

/// Key for the image-upload coalescing cache.
#[derive(Hash, Eq, PartialEq, Clone, Debug)]
pub struct ImageUploadCacheKey {
    program_url: String,
    predicate_type: u8,
    predicate_data: Vec<u8>,
}

impl ImageUploadCacheKey {
    pub fn new(program_url: &str, predicate: &crate::contracts::RequestPredicate) -> Self {
        Self {
            program_url: program_url.to_string(),
            predicate_type: predicate.predicateType as u8,
            predicate_data: predicate.data.to_vec(),
        }
    }
}

/// Cache that coalesces concurrent image uploads, keyed by [`ImageUploadCacheKey`].
pub type ImageUploadCache = Arc<Cache<ImageUploadCacheKey, String>>;

/// Backend-neutral request data needed to execute an evaluation/preflight.
#[derive(Clone, Debug)]
pub struct EvaluationRequest {
    pub request_id: String,
    pub program_url: String,
    pub selector: FixedBytes<4>,
    pub predicate: crate::contracts::RequestPredicate,
    pub input_type: crate::contracts::RequestInputType,
    pub input_data: Bytes,
    pub client_address: Address,
}

impl EvaluationRequest {
    pub fn from_order(order: &OrderRequest) -> Self {
        Self {
            request_id: order.id(),
            program_url: order.request.imageUrl.clone(),
            selector: order.request.requirements.selector,
            predicate: order.request.requirements.predicate.clone(),
            input_type: order.request.input.inputType,
            input_data: order.request.input.data.clone(),
            client_address: order.request.client_address(),
        }
    }

    pub fn cache_key(
        &self,
        program_id: String,
        max_cycles: u64,
    ) -> Result<PreflightCacheKey, OrderPricingError> {
        let predicate_data = self.predicate.data.to_vec();
        let input = match self.input_type {
            RequestInputType::Url => {
                let input_url = std::str::from_utf8(&self.input_data)
                    .context("input url is not utf8")
                    .map_err(|e| OrderPricingError::FetchInputErr(Arc::new(e)))?
                    .to_string();
                InputCacheKey::Url(input_url)
            }
            RequestInputType::Inline => {
                let mut hasher = Sha256::new();
                sha2::Digest::update(&mut hasher, &self.input_data);
                InputCacheKey::Hash(hasher.finalize().into())
            }
            RequestInputType::__Invalid => {
                return Err(OrderPricingError::UnexpectedErr(Arc::new(anyhow::anyhow!(
                    "Unknown input type: {:?}",
                    self.input_type
                ))));
            }
        };

        Ok(PreflightCacheKey {
            program_id,
            selector: self.selector,
            predicate_data,
            input,
            max_cycles,
        })
    }
}

/// Resource limits to apply while evaluating a request.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub struct EvaluationLimits {
    pub max_cycles: u64,
}

impl EvaluationLimits {
    pub fn new(max_cycles: u64) -> Self {
        Self { max_cycles }
    }
}

/// Executes request preflight for pricing.
#[async_trait]
pub trait RequestEvaluator {
    async fn evaluate_request(
        &self,
        request: EvaluationRequest,
        limits: EvaluationLimits,
    ) -> Result<RequestEvaluation, OrderPricingError>;
}

/// Predicate that flags priority requestors. Defaults to none if absent.
pub type PriorityRequestorCheck = Arc<dyn Fn(&Address) -> bool + Send + Sync>;

#[cfg(test)]
mod tests {
    use super::*;

    use crate::{contracts::Predicate, GuestEnv};

    fn test_request() -> EvaluationRequest {
        let predicate: crate::contracts::RequestPredicate =
            Predicate::DigestMatch(crate::Digest::ZERO, crate::Digest::ZERO).into();
        let stdin = GuestEnv::builder().build_vec().unwrap();
        EvaluationRequest {
            request_id: "test-1".into(),
            program_url: "file:///fake".into(),
            selector: FixedBytes::ZERO,
            predicate,
            input_type: RequestInputType::Inline,
            input_data: stdin.into(),
            client_address: Address::ZERO,
        }
    }

    #[test]
    fn cache_key_includes_max_cycles() {
        // A `LimitExceeded` result cached under a low limit keys differently from a
        // higher-limit evaluation.
        let request = test_request();
        let low = request.cache_key("program".into(), 100).unwrap();
        let high = request.cache_key("program".into(), 1_000_000).unwrap();
        assert_ne!(low, high, "max_cycles must be part of the preflight cache key");
        assert_eq!(low, request.cache_key("program".into(), 100).unwrap());
    }
}
