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
use hex::FromHex;
use moka::future::Cache;
use sha2::{Digest, Sha256};

use super::{OrderPricingError, OrderRequest};
use crate::{
    contracts::{Predicate, RequestInputType},
    input::GuestEnv,
    prover_utils::prover::ProverObj,
    storage::StorageDownloader,
};

/// Result of executing a request far enough to learn backend-specific facts needed by pricing.
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
/// The unit is defined by the evaluator implementation. For the current RISC0
/// evaluator this is the reported cycle count.
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
/// Broker pricing and capacity policy consume this value. Backend evaluators are
/// responsible for mapping their native work model into this normalized unit.
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
enum PreflightErrorKind {
    LimitExceeded,
    GuestPanicked,
}

fn classify_preflight_error(error: &str) -> Option<PreflightErrorKind> {
    if error.contains("Session limit exceeded")
        || error.contains("Execution stopped intentionally due to session limit")
    {
        Some(PreflightErrorKind::LimitExceeded)
    } else if error.contains("Guest panicked") || error.contains("GuestPanic") {
        Some(PreflightErrorKind::GuestPanicked)
    } else {
        None
    }
}

/// Value type for preflight cache.
///
/// Successful entries intentionally include the public output because pricing
/// always consumes it for output-size and predicate checks.
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
}

/// Cache for preflight results to avoid duplicate computations.
pub type PreflightCache = Arc<Cache<PreflightCacheKey, PreflightCacheValue>>;

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

    fn cache_key(&self, program_id: String) -> Result<PreflightCacheKey, OrderPricingError> {
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

        Ok(PreflightCacheKey { program_id, selector: self.selector, predicate_data, input })
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

/// Executes request preflight for pricing without making broker policy decisions.
///
/// This is the narrow backend-facing part of request evaluation. It returns execution facts
/// and output bytes; profitability, collateral, deadlines, and capacity remain in
/// [`super::OrderPricingContext`].
#[allow(async_fn_in_trait)]
pub trait RequestEvaluator {
    async fn evaluate_request(
        &self,
        request: EvaluationRequest,
        limits: EvaluationLimits,
    ) -> Result<RequestEvaluation, OrderPricingError>;
}

/// RISC0-backed request evaluation hooks.
///
/// Implementors provide the execution dependencies; the blanket [`RequestEvaluator`]
/// implementation below handles upload, cache coalescing, bounded preflight, and
/// journal retrieval exactly as the existing RISC0 path does.
pub trait Risc0RequestEvaluatorContext {
    /// Access to the prover for preflight operations.
    fn prover(&self) -> &ProverObj;

    /// Access to the downloader for fetching images and inputs.
    fn downloader(&self) -> Arc<dyn StorageDownloader + Send + Sync>;

    /// Cache for coalescing and reusing preflight results.
    fn preflight_cache(&self) -> &PreflightCache;

    /// Whether requestor-specific limits should be bypassed for this client.
    fn is_priority_requestor(&self, client_addr: &Address) -> bool;
}

impl<T> RequestEvaluator for T
where
    T: Risc0RequestEvaluatorContext + Sync,
{
    async fn evaluate_request(
        &self,
        request: EvaluationRequest,
        limits: EvaluationLimits,
    ) -> Result<RequestEvaluation, OrderPricingError> {
        let max_cycles = limits.max_cycles;
        let prover = Risc0RequestEvaluatorContext::prover(self).clone();
        let downloader = Risc0RequestEvaluatorContext::downloader(self);
        let cache = Risc0RequestEvaluatorContext::preflight_cache(self).clone();
        let request_id = request.request_id.clone();
        let program_url = request.program_url.clone();
        let predicate = request.predicate.clone();
        let cache_key = request.cache_key(
            upload_image_with_downloader(&prover, &program_url, &predicate, downloader.as_ref())
                .await
                .map_err(|e| OrderPricingError::FetchImageErr(Arc::new(e)))?,
        )?;
        let input_type = request.input_type;
        let input_data = request.input_data;
        let is_priority =
            Risc0RequestEvaluatorContext::is_priority_requestor(self, &request.client_address);

        loop {
            let program_id = cache_key.program_id.clone();
            // Multiple concurrent calls of this coalesce into a single execution.
            // https://docs.rs/moka/latest/moka/future/struct.Cache.html#concurrent-calls-on-the-same-key
            let result = cache
                .try_get_with(cache_key.clone(), async {
                    tracing::trace!(
                        "Starting preflight execution of {request_id} with limit of {max_cycles} cycles"
                    );

                    let input_id = upload_input_with_downloader(
                        &prover,
                        input_type,
                        &input_data,
                        downloader.as_ref(),
                        is_priority,
                    )
                    .await
                    .map_err(|e| OrderPricingError::FetchInputErr(Arc::new(e)))?;

                    match prover
                        .preflight(
                            &program_id,
                            &input_id,
                            vec![],
                            Some(max_cycles),
                            &request_id,
                        )
                        .await
                    {
                        Ok(res) => {
                            let stats = res.stats.ok_or_else(|| {
                                OrderPricingError::UnexpectedErr(Arc::new(anyhow::anyhow!(
                                    "Preflight execution of {request_id} succeeded but stats are missing"
                                )))
                            })?;
                            let evaluation_id = res.id;
                            let public_output = Risc0RequestEvaluatorContext::prover(self)
                                .get_preflight_journal(&evaluation_id)
                                .await
                                .map_err(|e| {
                                    OrderPricingError::UnexpectedErr(Arc::new(e.into()))
                                })?
                                .ok_or_else(|| {
                                    OrderPricingError::UnexpectedErr(Arc::new(anyhow::anyhow!(
                                        "Preflight journal not found"
                                    )))
                                })?;
                            tracing::debug!(
                                "Preflight execution of {request_id} with session id {} and {} mcycles completed",
                                evaluation_id,
                                stats.total_cycles / 1_000_000
                            );
                            Ok(RequestEvaluation::Success {
                                evaluation_id,
                                metrics: EvaluationMetrics::new(
                                    NativeWork::new(stats.total_cycles),
                                    NormalizedWork::new(stats.total_cycles),
                                ),
                                program_id,
                                input_id,
                                public_output,
                            })
                        }
                        Err(err) => {
                            let err_msg = err.to_string();
                            match classify_preflight_error(&err_msg) {
                                Some(PreflightErrorKind::LimitExceeded) => {
                                    tracing::debug!(
                                        "Skipping order {request_id} due to intentional execution limit of {max_cycles}"
                                    );
                                    Ok(RequestEvaluation::LimitExceeded { limit: limits })
                                }
                                Some(PreflightErrorKind::GuestPanicked) => {
                                    tracing::debug!(
                                        "Skipping order {request_id} due to guest panic: {}",
                                        err_msg
                                    );
                                    Ok(RequestEvaluation::GuestFailed)
                                }
                                None => Err(OrderPricingError::UnexpectedErr(Arc::new(err.into()))),
                            }
                        }
                    }
                })
                .await
                .map_err(|e| (*e).clone())?;

            if let RequestEvaluation::LimitExceeded { limit } = result {
                if limit.max_cycles < max_cycles {
                    cache.invalidate(&cache_key).await;
                    continue;
                }
                return Ok(RequestEvaluation::LimitExceeded { limit });
            }

            return Ok(result);
        }
    }
}

/// Upload an image to the prover using the provided downloader.
///
/// This is a standalone function (not a trait method) so it can be called from inside
/// async closures like `try_get_with` without capturing `&self`.
pub(super) async fn upload_image_with_downloader(
    prover: &ProverObj,
    image_url: &str,
    predicate: &crate::contracts::RequestPredicate,
    downloader: &(dyn StorageDownloader + Send + Sync),
) -> anyhow::Result<String> {
    let predicate = Predicate::try_from(predicate.clone()).context("Failed to parse predicate")?;

    let image_id_str = predicate.image_id().map(|image_id| image_id.to_string());

    if let Some(ref image_id_str) = image_id_str {
        if prover.has_image(image_id_str).await? {
            tracing::debug!("Skipping program upload for cached image ID: {image_id_str}");
            return Ok(image_id_str.clone());
        }
    }

    tracing::debug!("Fetching program from URI {image_url}");
    let image_data = downloader
        .download(image_url)
        .await
        .with_context(|| format!("Failed to fetch image URI: {image_url}"))?;

    let image_id =
        risc0_zkvm::compute_image_id(&image_data).context("Failed to compute image ID")?;

    if let Some(ref expected_image_id_str) = image_id_str {
        let expected_image_id = risc0_zkvm::sha::Digest::from_hex(expected_image_id_str)?;
        if image_id != expected_image_id {
            anyhow::bail!(
                "image ID does not match requirements; expect {}, got {}",
                expected_image_id,
                image_id
            );
        }
    }

    let image_id_str = image_id.to_string();

    tracing::debug!("Uploading program with image ID {image_id_str} to prover");
    prover.upload_image(&image_id_str, image_data).await?;

    Ok(image_id_str)
}

/// Upload input data to the prover (from inline data or URL) using the provided downloader.
///
/// This is a standalone function (not a trait method) so it can be called from inside
/// async closures like `try_get_with` without capturing `&self`.
///
/// If `is_priority_requestor` is true, size limits are bypassed when fetching from URLs.
pub(super) async fn upload_input_with_downloader(
    prover: &ProverObj,
    input_type: crate::contracts::RequestInputType,
    input_data: &Bytes,
    downloader: &(dyn StorageDownloader + Send + Sync),
    is_priority_requestor: bool,
) -> anyhow::Result<String> {
    match input_type {
        crate::contracts::RequestInputType::Inline => {
            let stdin = GuestEnv::decode(input_data).context("Failed to decode input")?.stdin;
            prover.upload_input(stdin).await.map_err(|e| anyhow::anyhow!("{}", e))
        }
        crate::contracts::RequestInputType::Url => {
            let input_url =
                std::str::from_utf8(input_data).context("input url is not valid utf8")?;

            tracing::debug!("Fetching input from URI {input_url}");
            let raw_input = if is_priority_requestor {
                downloader.download_with_limit(input_url, usize::MAX).await
            } else {
                downloader.download(input_url).await
            }
            .with_context(|| format!("Failed to fetch input URI: {input_url}"))?;

            let stdin =
                GuestEnv::decode(&raw_input).context("Failed to decode input from URL")?.stdin;

            prover.upload_input(stdin).await.map_err(|e| anyhow::anyhow!("{}", e))
        }
        crate::contracts::RequestInputType::__Invalid => {
            anyhow::bail!("Invalid input type")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_preflight_limit_errors() {
        assert_eq!(
            classify_preflight_error("Session limit exceeded after 100 cycles"),
            Some(PreflightErrorKind::LimitExceeded)
        );
        assert_eq!(
            classify_preflight_error("Execution stopped intentionally due to session limit"),
            Some(PreflightErrorKind::LimitExceeded)
        );
    }

    #[test]
    fn classifies_preflight_guest_panic_errors() {
        assert_eq!(
            classify_preflight_error("Guest panicked: assertion failed"),
            Some(PreflightErrorKind::GuestPanicked)
        );
        assert_eq!(
            classify_preflight_error("GuestPanic at pc 0x1234"),
            Some(PreflightErrorKind::GuestPanicked)
        );
    }

    #[test]
    fn leaves_unknown_preflight_errors_unclassified() {
        assert_eq!(classify_preflight_error("network unavailable"), None);
    }

    use crate::{
        prover_utils::prover::{ExecutorResp, ProofResult, Prover, ProverError},
        storage::{StorageDownloader, StorageError},
    };
    use async_trait::async_trait;
    use risc0_zkvm::{sha::Digest as Risc0Digest, Receipt};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use url::Url;

    struct StubProver {
        preflight_calls: AtomicUsize,
    }

    impl StubProver {
        fn new() -> Arc<Self> {
            Arc::new(Self { preflight_calls: AtomicUsize::new(0) })
        }
    }

    #[async_trait]
    impl Prover for StubProver {
        async fn has_image(&self, _: &str) -> Result<bool, ProverError> {
            Ok(true)
        }
        async fn upload_input(&self, _: Vec<u8>) -> Result<String, ProverError> {
            Ok("input-id".to_string())
        }
        async fn upload_image(&self, _: &str, _: Vec<u8>) -> Result<(), ProverError> {
            unreachable!("has_image short-circuits the upload path")
        }
        async fn preflight(
            &self,
            _image_id: &str,
            _input_id: &str,
            _assumptions: Vec<String>,
            _limit: Option<u64>,
            _order_id: &str,
        ) -> Result<ProofResult, ProverError> {
            let call = self.preflight_calls.fetch_add(1, Ordering::SeqCst);
            if call == 0 {
                Err(ProverError::ProvingFailed("Session limit exceeded after 100 cycles".into()))
            } else {
                Ok(ProofResult {
                    id: format!("eval-{call}"),
                    stats: Some(ExecutorResp { total_cycles: 50, ..Default::default() }),
                    elapsed_time: 0.0,
                })
            }
        }
        async fn prove_stark(
            &self,
            _: &str,
            _: &str,
            _: Vec<String>,
        ) -> Result<String, ProverError> {
            unreachable!()
        }
        async fn wait_for_stark(&self, _: &str) -> Result<ProofResult, ProverError> {
            unreachable!()
        }
        async fn cancel_stark(&self, _: &str) -> Result<(), ProverError> {
            unreachable!()
        }
        async fn get_receipt(&self, _: &str) -> Result<Option<Receipt>, ProverError> {
            unreachable!()
        }
        async fn get_preflight_journal(&self, _: &str) -> Result<Option<Vec<u8>>, ProverError> {
            Ok(Some(vec![]))
        }
        async fn get_journal(&self, _: &str) -> Result<Option<Vec<u8>>, ProverError> {
            unreachable!()
        }
        async fn compress(&self, _: &str) -> Result<String, ProverError> {
            unreachable!()
        }
        async fn get_compressed_receipt(&self, _: &str) -> Result<Option<Vec<u8>>, ProverError> {
            unreachable!()
        }
        async fn compress_blake3_groth16(&self, _: &str) -> Result<String, ProverError> {
            unreachable!()
        }
        async fn get_blake3_groth16_receipt(
            &self,
            _: &str,
        ) -> Result<Option<Vec<u8>>, ProverError> {
            unreachable!()
        }
    }

    struct NoopDownloader;

    #[async_trait]
    impl StorageDownloader for NoopDownloader {
        async fn download_url_with_limit(&self, _: Url, _: usize) -> Result<Vec<u8>, StorageError> {
            unreachable!("inline input + cached image — downloader should never be called")
        }
        async fn download_url(&self, _: Url) -> Result<Vec<u8>, StorageError> {
            unreachable!("inline input + cached image — downloader should never be called")
        }
    }

    struct StubCtx {
        prover: ProverObj,
        downloader: Arc<dyn StorageDownloader + Send + Sync>,
        cache: PreflightCache,
    }

    impl Risc0RequestEvaluatorContext for StubCtx {
        fn prover(&self) -> &ProverObj {
            &self.prover
        }
        fn downloader(&self) -> Arc<dyn StorageDownloader + Send + Sync> {
            self.downloader.clone()
        }
        fn preflight_cache(&self) -> &PreflightCache {
            &self.cache
        }
        fn is_priority_requestor(&self, _: &Address) -> bool {
            false
        }
    }

    fn test_request() -> EvaluationRequest {
        let predicate: crate::contracts::RequestPredicate =
            Predicate::DigestMatch(Risc0Digest::ZERO, Risc0Digest::ZERO).into();
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

    #[tokio::test]
    async fn invalidation_loop_retries_after_cached_limit_exceeded() {
        let stub = StubProver::new();
        let ctx = StubCtx {
            prover: stub.clone(),
            downloader: Arc::new(NoopDownloader),
            cache: Arc::new(Cache::new(64)),
        };

        let r1 = ctx
            .evaluate_request(test_request(), EvaluationLimits::new(100))
            .await
            .expect("first call should classify the prover error, not propagate it");
        assert!(
            matches!(r1, RequestEvaluation::LimitExceeded { limit } if limit.max_cycles == 100),
            "expected LimitExceeded with cached limit=100, got {r1:?}"
        );
        assert_eq!(stub.preflight_calls.load(Ordering::SeqCst), 1);

        let r2 = ctx
            .evaluate_request(test_request(), EvaluationLimits::new(1_000_000))
            .await
            .expect("retry after invalidation should succeed");
        assert!(
            matches!(r2, RequestEvaluation::Success { .. }),
            "expected Success after cache invalidation + retry, got {r2:?}"
        );
        assert_eq!(
            stub.preflight_calls.load(Ordering::SeqCst),
            2,
            "the relaxed-limit call must re-run preflight, not return the cached LimitExceeded"
        );
    }

    #[tokio::test]
    async fn cached_limit_exceeded_returned_when_caller_limit_unchanged() {
        let stub = StubProver::new();
        let ctx = StubCtx {
            prover: stub.clone(),
            downloader: Arc::new(NoopDownloader),
            cache: Arc::new(Cache::new(64)),
        };

        let r1 = ctx.evaluate_request(test_request(), EvaluationLimits::new(100)).await.unwrap();
        assert!(matches!(r1, RequestEvaluation::LimitExceeded { .. }));

        let r2 = ctx.evaluate_request(test_request(), EvaluationLimits::new(100)).await.unwrap();
        assert!(matches!(r2, RequestEvaluation::LimitExceeded { .. }));
        assert_eq!(
            stub.preflight_calls.load(Ordering::SeqCst),
            1,
            "same-limit retry must hit the cache, not invalidate"
        );
    }
}
