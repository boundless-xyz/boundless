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

use alloy::primitives::Bytes;
use anyhow::Context as _;
use async_trait::async_trait;
use hex::FromHex as _;

use crate::{
    contracts::Predicate,
    input::GuestEnv,
    prover_utils::{
        prover::ProverObj, request_evaluator::PreflightErrorKind, EvaluationLimits,
        EvaluationMetrics, EvaluationRequest, ImageUploadCache, ImageUploadCacheKey, NativeWork,
        NormalizedWork, OrderPricingError, PreflightCache, PriorityRequestorCheck,
        RequestEvaluation, RequestEvaluator,
    },
    storage::StorageDownloader,
};

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

/// Concrete RISC0 preflight pipeline.
#[derive(Clone)]
pub struct Risc0Evaluator {
    prover: ProverObj,
    downloader: Arc<dyn StorageDownloader + Send + Sync>,
    preflight_cache: PreflightCache,
    image_upload_cache: Option<ImageUploadCache>,
    priority_check: Option<PriorityRequestorCheck>,
}

impl Risc0Evaluator {
    /// Create a new `Risc0Evaluator`
    pub fn new(
        prover: ProverObj,
        downloader: Arc<dyn StorageDownloader + Send + Sync>,
        preflight_cache: PreflightCache,
    ) -> Self {
        Self { prover, downloader, preflight_cache, image_upload_cache: None, priority_check: None }
    }

    /// Enable coalescing of concurrent image uploads for identical requests.
    #[cfg(any(feature = "prover_utils", test))]
    pub fn with_image_upload_cache(mut self, cache: ImageUploadCache) -> Self {
        self.image_upload_cache = Some(cache);
        self
    }

    /// Install a priority-requestor predicate.
    #[cfg(feature = "prover_utils")]
    pub fn with_priority_check(mut self, check: PriorityRequestorCheck) -> Self {
        self.priority_check = Some(check);
        self
    }

    async fn resolve_program_id(
        &self,
        program_url: &str,
        predicate: &crate::contracts::RequestPredicate,
    ) -> Result<String, OrderPricingError> {
        let Some(cache) = self.image_upload_cache.as_ref() else {
            return upload_image_with_downloader(
                &self.prover,
                program_url,
                predicate,
                self.downloader.as_ref(),
            )
            .await
            .map_err(|e| OrderPricingError::FetchImageErr(Arc::new(e)));
        };
        // Multiple concurrent calls on the same key coalesce into a single upload.
        // https://docs.rs/moka/latest/moka/future/struct.Cache.html#concurrent-calls-on-the-same-key
        let key = ImageUploadCacheKey::new(program_url, predicate);
        let prover = self.prover.clone();
        let program_url = program_url.to_string();
        let predicate = predicate.clone();
        let downloader = self.downloader.clone();
        cache
            .try_get_with(key, async move {
                upload_image_with_downloader(&prover, &program_url, &predicate, downloader.as_ref())
                    .await
                    .map_err(|e| OrderPricingError::FetchImageErr(Arc::new(e)))
            })
            .await
            .map_err(|e| (*e).clone())
    }
}

#[async_trait]
impl RequestEvaluator for Risc0Evaluator {
    async fn evaluate_request(
        &self,
        request: EvaluationRequest,
        limits: EvaluationLimits,
    ) -> Result<RequestEvaluation, OrderPricingError> {
        let max_cycles = limits.max_cycles;
        let prover = self.prover.clone();
        let downloader = self.downloader.clone();
        let cache = self.preflight_cache.clone();
        let request_id = request.request_id.clone();
        let program_url = request.program_url.clone();
        let predicate = request.predicate.clone();
        let program_id = self.resolve_program_id(&program_url, &predicate).await?;
        let cache_key = request.cache_key(program_id, max_cycles)?;
        let input_type = request.input_type;
        let input_data = request.input_data;
        let is_priority = self
            .priority_check
            .as_ref()
            .map(|check| check(&request.client_address))
            .unwrap_or(false);

        let program_id = cache_key.program_id.clone();
        // Concurrent calls on the same key coalesce into a single execution.
        // https://docs.rs/moka/latest/moka/future/struct.Cache.html#concurrent-calls-on-the-same-key
        cache
            .try_get_with(cache_key, async {
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
                    .preflight(&program_id, &input_id, vec![], Some(max_cycles), &request_id)
                    .await
                {
                    Ok(res) => {
                        let stats = res.stats.ok_or_else(|| {
                            OrderPricingError::UnexpectedErr(Arc::new(anyhow::anyhow!(
                                "Preflight execution of {request_id} succeeded but stats are missing"
                            )))
                        })?;
                        let evaluation_id = res.id;
                        let public_output = prover
                            .get_preflight_journal(&evaluation_id)
                            .await
                            .map_err(|e| OrderPricingError::UnexpectedErr(Arc::new(e.into())))?
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
            .map_err(|e| (*e).clone())
    }
}

/// Upload an image to the prover using the provided downloader.
async fn upload_image_with_downloader(
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
/// If `is_priority_requestor` is true, size limits are bypassed when fetching from URLs.
async fn upload_input_with_downloader(
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

    use crate::{
        contracts::RequestInputType,
        prover_utils::prover::{ExecutorResp, ProofResult, Prover, ProverError},
        storage::{StorageDownloader, StorageError},
    };
    use alloy_primitives::{Address, FixedBytes};
    use async_trait::async_trait;
    use moka::future::Cache;
    use risc0_zkvm::Receipt;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use url::Url;

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

    struct StubProver {
        preflight_calls: AtomicUsize,
        has_image_calls: AtomicUsize,
    }

    impl StubProver {
        fn new() -> Arc<Self> {
            Arc::new(Self {
                preflight_calls: AtomicUsize::new(0),
                has_image_calls: AtomicUsize::new(0),
            })
        }
    }

    #[async_trait]
    impl Prover for StubProver {
        async fn has_image(&self, _: &str) -> Result<bool, ProverError> {
            self.has_image_calls.fetch_add(1, Ordering::SeqCst);
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

    fn stub_evaluator(prover: ProverObj) -> Risc0Evaluator {
        Risc0Evaluator::new(prover, Arc::new(NoopDownloader), Arc::new(Cache::new(64)))
    }

    fn stub_evaluator_with_image_cache(prover: ProverObj) -> Risc0Evaluator {
        stub_evaluator(prover).with_image_upload_cache(Arc::new(Cache::new(64)))
    }

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

    #[tokio::test]
    async fn higher_limit_caller_reevaluates_past_cached_limit_exceeded() {
        let stub = StubProver::new();
        let ctx = stub_evaluator(stub.clone());

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
            .expect("higher-limit call should miss the cache and re-evaluate");
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
        let ctx = stub_evaluator(stub.clone());

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

    #[tokio::test]
    async fn image_upload_cache_coalesces_repeated_evaluations() {
        // With an image-upload cache, identical requests resolve their program once.
        let stub = StubProver::new();
        let ctx = stub_evaluator_with_image_cache(stub.clone());

        for _ in 0..3 {
            ctx.evaluate_request(test_request(), EvaluationLimits::new(100)).await.unwrap();
        }

        assert_eq!(
            stub.has_image_calls.load(Ordering::SeqCst),
            1,
            "image upload must be coalesced across identical evaluations"
        );
    }

    #[tokio::test]
    async fn image_upload_runs_per_call_without_a_cache() {
        // Without an image-upload cache, every evaluation uploads independently.
        let stub = StubProver::new();
        let ctx = stub_evaluator(stub.clone());

        for _ in 0..3 {
            ctx.evaluate_request(test_request(), EvaluationLimits::new(100)).await.unwrap();
        }

        assert_eq!(stub.has_image_calls.load(Ordering::SeqCst), 3);
    }
}
