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

use std::{path::PathBuf, sync::Arc};

use alloy::sol_types::{SolStruct, SolValue};
use alloy::{
    network::Ethereum,
    primitives::{Address, FixedBytes, B256, U256},
    providers::{DynProvider, Provider},
};
use async_trait::async_trait;
use blake3_groth16::Blake3Groth16Receipt;
use boundless_assessor::{AssessorInput, Fulfillment};
use boundless_market::{
    contracts::{
        boundless_market::BoundlessMarketService, eip712_domain, encode_seal, AssessorJournal,
        Fulfillment as MarketFulfillment, FulfillmentData, FulfillmentDataImageIdAndJournal,
        FulfillmentDataType, Predicate, PredicateType, RequestInputType, UNSPECIFIED_SELECTOR,
    },
    input::GuestEnv,
    prover_utils::{
        EvaluationLimits, EvaluationRequest, ImageUploadCache, OrderPricingError, PreflightCache,
        PriorityRequestorCheck, RequestEvaluation, RequestEvaluator, Risc0Evaluator,
    },
    selector::{is_blake3_groth16_selector, is_groth16_selector, ProofType, SelectorExt},
    storage::StorageDownloader,
    Deployment,
};
use hex::FromHex;
use risc0_aggregation::{GuestState, SetInclusionReceipt, SetInclusionReceiptVerifierParameters};
use risc0_ethereum_contracts::set_verifier::SetVerifierService;
use risc0_zkvm::{
    compute_image_id,
    sha::{Digest as Risc0Digest, Digestible},
    MaybePruned, Receipt, ReceiptClaim,
};

const PREFLIGHT_CACHE_SIZE: u64 = 5000;
const PREFLIGHT_CACHE_TTL_SECS: u64 = 3 * 60 * 60;
const IMAGE_UPLOAD_CACHE_SIZE: u64 = 1000;
const RISC0_V3_BACKEND_ID: &str = "risc0_v3";

const SELECTOR_GROTH16_V3_0: FixedBytes<4> =
    FixedBytes::new((SelectorExt::Groth16V3_0 as u32).to_be_bytes());
const SELECTOR_BLAKE3_GROTH16_V0_1: FixedBytes<4> =
    FixedBytes::new((SelectorExt::Blake3Groth16V0_1 as u32).to_be_bytes());
const SELECTOR_FAKE_RECEIPT: FixedBytes<4> =
    FixedBytes::new((SelectorExt::FakeReceipt as u32).to_be_bytes());
const SELECTOR_FAKE_BLAKE3_GROTH16: FixedBytes<4> =
    FixedBytes::new((SelectorExt::FakeBlake3Groth16 as u32).to_be_bytes());

use crate::{
    config::ConfigLock,
    futures_retry::retry_with_context,
    is_dev_mode,
    provers::{self, ProverObj},
    requestor_monitor::PriorityRequestors,
    ConfigurableDownloader,
};
use anyhow::{Context, Result};

use super::types::{
    AssessorArtifact, Backend, BackendBatchState, BackendError, BackendId, BackendOrderState,
    BatchClose, BatchProcessor, BatchProcessorObj, BatchSizeEstimate, BatchSizeEstimateRequest,
    BatchUpdate, CancelOrder, ClaimDigest, CloseBatch, Digest as BackendDigest,
    FailedFulfillmentOrder, FulfillmentBatch, OrderFulfillmentArtifact, OrderProcessProgress,
    OrderProvingData, ProcessOrder, ProcessedOrder, ProofId, SubmissionAssessorArtifact,
    SubmissionPath, SubmissionPlan, UpdateBatch, VerifierUpdate, VerifierUpdateError,
};

/// Bump when the serialized [`Risc0OrderState`] shape changes incompatibly. A newer-than-known
/// version on read means the broker was downgraded mid-flight; [`Risc0OrderState::decode`] rejects
/// it rather than silently misreading. State written before versioning has no field and defaults
/// to 1, which is its shape.
const RISC0_ORDER_STATE_VERSION: u32 = 1;

fn risc0_order_state_version() -> u32 {
    RISC0_ORDER_STATE_VERSION
}

/// JSON-serialized inside [`BackendOrderState`] for the RISC0 backend.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub(super) struct Risc0OrderState {
    #[serde(default = "risc0_order_state_version")]
    pub(super) version: u32,
    pub(super) proof_id: String,
    #[serde(default)]
    pub(super) compressed_proof_id: Option<String>,
}

impl Default for Risc0OrderState {
    fn default() -> Self {
        Self {
            version: RISC0_ORDER_STATE_VERSION,
            proof_id: String::new(),
            compressed_proof_id: None,
        }
    }
}

impl Risc0OrderState {
    pub(super) fn decode(state: &BackendOrderState) -> Result<Self> {
        let decoded: Self = serde_json::from_value(state.0.clone())
            .context("Failed to decode RISC0 order state")?;
        anyhow::ensure!(
            decoded.version <= RISC0_ORDER_STATE_VERSION,
            "RISC0 order state schema version {} is newer than this broker supports ({}); \
             the broker may have been downgraded while an order was in flight",
            decoded.version,
            RISC0_ORDER_STATE_VERSION,
        );
        Ok(decoded)
    }

    pub(super) fn encode(&self) -> BackendOrderState {
        BackendOrderState(
            serde_json::to_value(self).expect("Risc0OrderState always serializes to JSON"),
        )
    }
}

mod batch;

pub use batch::prune_receipt_claim_journal;
use batch::{Risc0BatchProcessor, Risc0BatchState, Risc0Submission};

pub struct Risc0Backend {
    id: BackendId,
    prover: ProverObj,
    snark_prover: ProverObj,
    downloader: ConfigurableDownloader,
    priority_requestors: PriorityRequestors,
    evaluator: Risc0Evaluator,
    set_builder_program_id: Option<Risc0Digest>,
    set_verifier_addr: Option<Address>,
    set_verifier: Option<SetVerifierService<DynProvider>>,
    batch_processor: Option<BatchProcessorObj>,
}

impl Risc0Backend {
    pub fn default_id() -> BackendId {
        BackendId::new(RISC0_V3_BACKEND_ID)
    }

    /// Production constructor: builds the prover backends from broker config.
    pub fn new(
        config: ConfigLock,
        bonsai_api_key: Option<&str>,
        bonsai_api_url: Option<&url::Url>,
        bento_api_url: Option<&url::Url>,
        downloader: ConfigurableDownloader,
        priority_requestors: PriorityRequestors,
    ) -> Result<Self> {
        let (prover, snark_prover) =
            Self::build_provers(&config, bonsai_api_key, bonsai_api_url, bento_api_url)?;
        Ok(Self::with_provers(prover, snark_prover, downloader, priority_requestors))
    }

    /// Constructor that takes the prover backends explicitly. Used by tests.
    pub fn with_provers(
        prover: ProverObj,
        snark_prover: ProverObj,
        downloader: ConfigurableDownloader,
        priority_requestors: PriorityRequestors,
    ) -> Self {
        let preflight_cache: PreflightCache = std::sync::Arc::new(
            moka::future::Cache::builder()
                .eviction_policy(moka::policy::EvictionPolicy::lru())
                .max_capacity(PREFLIGHT_CACHE_SIZE)
                .time_to_live(std::time::Duration::from_secs(PREFLIGHT_CACHE_TTL_SECS))
                .build(),
        );
        let image_upload_cache: ImageUploadCache = std::sync::Arc::new(
            moka::future::Cache::builder()
                .eviction_policy(moka::policy::EvictionPolicy::lru())
                .max_capacity(IMAGE_UPLOAD_CACHE_SIZE)
                .time_to_live(std::time::Duration::from_secs(PREFLIGHT_CACHE_TTL_SECS))
                .build(),
        );
        let priority_for_check = priority_requestors.clone();
        let priority_check: PriorityRequestorCheck =
            std::sync::Arc::new(move |addr| priority_for_check.is_priority_requestor(addr));
        let evaluator = Risc0Evaluator::new(
            prover.clone(),
            std::sync::Arc::new(downloader.clone()),
            preflight_cache,
        )
        .with_image_upload_cache(image_upload_cache)
        .with_priority_check(priority_check);

        Self {
            id: Self::default_id(),
            prover,
            snark_prover,
            downloader,
            priority_requestors,
            evaluator,
            set_builder_program_id: None,
            set_verifier_addr: None,
            set_verifier: None,
            batch_processor: None,
        }
    }

    fn build_provers(
        config: &ConfigLock,
        bonsai_api_key: Option<&str>,
        bonsai_api_url: Option<&url::Url>,
        bento_api_url: Option<&url::Url>,
    ) -> Result<(ProverObj, ProverObj)> {
        if is_dev_mode() {
            tracing::warn!(
                "WARNING: Running in dev mode does not generate valid receipts. \
                 Receipts generated from this process are invalid and should never be used in production."
            );
            let prover: ProverObj = Arc::new(provers::DefaultProver::new());
            return Ok((Arc::clone(&prover), prover));
        }
        if let (Some(key), Some(url)) = (bonsai_api_key, bonsai_api_url) {
            tracing::info!("Configured to run with Bonsai backend");
            let prover: ProverObj = Arc::new(
                provers::Bonsai::new(config.clone(), url.as_ref(), key)
                    .context("Failed to construct Bonsai client")?,
            );
            return Ok((Arc::clone(&prover), prover));
        }
        if let Some(url) = bento_api_url {
            tracing::info!("Configured to run with Bento backend");
            let prover: ProverObj = Arc::new(
                provers::Bonsai::new(config.clone(), url.as_ref(), "v1:reserved:1000")
                    .context("Failed to initialize Bento client")?,
            );
            let snark_prover: ProverObj = Arc::new(
                provers::Bonsai::new(config.clone(), url.as_ref(), "v1:reserved:2000")
                    .context("Failed to initialize Bento client")?,
            );
            return Ok((prover, snark_prover));
        }
        let prover: ProverObj = Arc::new(provers::DefaultProver::new());
        Ok((Arc::clone(&prover), prover))
    }

    fn selectors() -> Vec<FixedBytes<4>> {
        Self::selectors_for(is_dev_mode())
    }

    /// Verifier selectors this backend serves. `dev_mode` adds the dev-only fake-proof
    /// selectors; production selector routing uses only the non-dev set.
    fn selectors_for(dev_mode: bool) -> Vec<FixedBytes<4>> {
        let mut selectors =
            vec![UNSPECIFIED_SELECTOR, SELECTOR_GROTH16_V3_0, SELECTOR_BLAKE3_GROTH16_V0_1];
        if dev_mode {
            selectors.push(SELECTOR_FAKE_RECEIPT);
            selectors.push(SELECTOR_FAKE_BLAKE3_GROTH16);
        }
        selectors
    }

    #[cfg(test)]
    pub fn with_set_builder_program_id(mut self, set_builder_program_id: Risc0Digest) -> Self {
        self.set_builder_program_id = Some(set_builder_program_id);
        self
    }

    #[cfg(test)]
    pub fn with_set_verifier<P>(
        mut self,
        set_verifier_addr: Address,
        provider: Arc<P>,
        caller: Address,
    ) -> Self
    where
        P: Provider<Ethereum> + Clone + 'static,
    {
        self.set_verifier_addr = Some(set_verifier_addr);
        self.set_verifier =
            Some(SetVerifierService::new(set_verifier_addr, DynProvider::new(provider), caller));
        self
    }

    #[cfg(test)]
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn with_test_batch_processor(
        mut self,
        config: ConfigLock,
        prover: ProverObj,
        set_builder_guest_id: Risc0Digest,
        assessor_guest_id: Risc0Digest,
        market_addr: Address,
        prover_addr: Address,
        chain_id: u64,
    ) -> Self {
        let batch_processor = Arc::new(Risc0BatchProcessor::new(
            config,
            prover,
            set_builder_guest_id,
            assessor_guest_id,
            market_addr,
            prover_addr,
            chain_id,
        ));
        self.batch_processor = Some(batch_processor);
        self
    }

    pub async fn with_batch_processor_from_deployment<P>(
        mut self,
        config: ConfigLock,
        provider: &Arc<P>,
        deployment: &Deployment,
        prover_addr: Address,
        chain_id: u64,
    ) -> Result<Self>
    where
        P: Provider<Ethereum> + Clone + 'static,
    {
        let set_builder_img_id =
            self.fetch_and_upload_set_builder_image(provider, deployment, &config).await?;
        let assessor_img_id =
            self.fetch_and_upload_assessor_image(provider, deployment, &config).await?;

        let txn_timeout = {
            let cfg = config.lock_all().context("Failed to lock config")?;
            cfg.batcher.txn_timeout
        };
        let set_verifier = SetVerifierService::new(
            deployment.set_verifier_address,
            DynProvider::new(provider.clone()),
            prover_addr,
        )
        .with_timeout(std::time::Duration::from_secs(txn_timeout));

        let batch_processor = Arc::new(Risc0BatchProcessor::new(
            config,
            self.snark_prover.clone(),
            set_builder_img_id,
            assessor_img_id,
            deployment.boundless_market_address,
            prover_addr,
            chain_id,
        ));

        self.set_builder_program_id = Some(set_builder_img_id);
        self.set_verifier_addr = Some(deployment.set_verifier_address);
        self.set_verifier = Some(set_verifier);
        self.batch_processor = Some(batch_processor);
        Ok(self)
    }

    fn require_set_verifier(&self, verifier: Address) -> Result<&SetVerifierService<DynProvider>> {
        let set_verifier =
            self.set_verifier.as_ref().context("RISC0 backend is missing set verifier")?;
        anyhow::ensure!(
            self.set_verifier_addr == Some(verifier),
            "verifier update targets {verifier}, but RISC0 backend is configured for {:?}",
            self.set_verifier_addr,
        );
        Ok(set_verifier)
    }

    async fn fetch_and_upload_set_builder_image<P>(
        &self,
        provider: &Arc<P>,
        deployment: &Deployment,
        config: &ConfigLock,
    ) -> Result<Risc0Digest>
    where
        P: Provider<Ethereum> + Clone + 'static,
    {
        let set_verifier_contract = SetVerifierService::new(
            deployment.set_verifier_address,
            provider.clone(),
            Address::ZERO,
        );

        let (image_id, image_url_str) = set_verifier_contract
            .image_info()
            .await
            .context("Failed to get set builder image_info")?;
        let image_id = Risc0Digest::from_bytes(image_id.0);
        let (path, default_url) = {
            let config = config.lock_all().context("Failed to lock config")?;
            (
                config.prover.set_builder_guest_path.clone(),
                config.market.set_builder_default_image_url.clone(),
            )
        };

        self.fetch_and_upload_image("set builder", image_id, image_url_str, path, default_url)
            .await
            .context("uploading set builder image")?;
        Ok(image_id)
    }

    async fn fetch_and_upload_assessor_image<P>(
        &self,
        provider: &Arc<P>,
        deployment: &Deployment,
        config: &ConfigLock,
    ) -> Result<Risc0Digest>
    where
        P: Provider<Ethereum> + Clone + 'static,
    {
        let boundless_market = BoundlessMarketService::new_for_broker(
            deployment.boundless_market_address,
            provider.clone(),
            Address::ZERO,
        );
        let (image_id, image_url_str) =
            boundless_market.image_info().await.context("Failed to get assessor image_info")?;
        let image_id = Risc0Digest::from_bytes(image_id.0);

        let (path, default_url) = {
            let config = config.lock_all().context("Failed to lock config")?;
            (
                config.prover.assessor_set_guest_path.clone(),
                config.market.assessor_default_image_url.clone(),
            )
        };

        self.fetch_and_upload_image("assessor", image_id, image_url_str, path, default_url)
            .await
            .context("uploading assessor image")?;
        Ok(image_id)
    }

    async fn fetch_and_upload_image(
        &self,
        image_label: &'static str,
        image_id: Risc0Digest,
        contract_url: String,
        program_path: Option<PathBuf>,
        default_url: String,
    ) -> Result<()> {
        if self.snark_prover.has_image(&image_id.to_string()).await? {
            tracing::debug!("{} image {} already uploaded, skipping pull", image_label, image_id);
            return Ok(());
        }

        tracing::debug!("Fetching {} image", image_label);
        let program_bytes = if let Some(path) = program_path {
            tokio::fs::read(&path)
                .await
                .with_context(|| format!("Failed to read program file: {}", path.display()))?
        } else {
            match self.download_image(&default_url, "default").await {
                Ok(bytes) => {
                    let computed_id =
                        compute_image_id(&bytes).context("Failed to compute image ID")?;
                    if computed_id == image_id {
                        tracing::debug!(
                            "Successfully verified {} image from default URL",
                            image_label
                        );
                        bytes
                    } else {
                        tracing::warn!(
                            "{} image ID mismatch from default URL: expected {}, got {}, falling back to contract URL",
                            image_label,
                            image_id,
                            computed_id
                        );
                        self.download_image(&contract_url, "contract").await?
                    }
                }
                Err(e) => {
                    tracing::warn!(
                        "Failed to download {} image from default URL: {}, falling back to contract URL",
                        image_label,
                        e
                    );
                    self.download_image(&contract_url, "contract").await?
                }
            }
        };

        let computed_id = compute_image_id(&program_bytes).context("Failed to compute image ID")?;

        if computed_id != image_id {
            anyhow::bail!(
                "{} image ID mismatch: expected {}, got {}",
                image_label,
                image_id,
                computed_id
            );
        }

        tracing::debug!("Uploading {} image to bento", image_label);
        self.snark_prover
            .upload_image(&image_id.to_string(), program_bytes)
            .await
            .with_context(|| format!("Failed to upload {} image to prover", image_label))?;
        Ok(())
    }

    async fn download_image(&self, url: &str, source_name: &str) -> Result<Vec<u8>> {
        tracing::trace!("Attempting to download image from {}: {}", source_name, url);

        let bytes = self
            .downloader
            .download(url)
            .await
            .with_context(|| format!("Failed to download image from {}", source_name))?;

        tracing::trace!("Successfully downloaded image from {}", source_name);
        Ok(bytes)
    }

    async fn upload_order_image(&self, request: &crate::ProofRequest) -> Result<String> {
        let predicate = Predicate::try_from(request.requirements.predicate.clone())
            .with_context(|| format!("Failed to parse predicate for request {:x}", request.id))?;

        let image_id_str = predicate.image_id().map(|image_id| image_id.to_string());

        // Claim-digest predicates do not carry an image id, so the image must be downloaded before
        // the RISC0 image id can be computed and uploaded.
        if let Some(ref image_id_str) = image_id_str {
            if self.prover.has_image(image_id_str).await? {
                tracing::debug!(
                    "Skipping program upload for cached image ID: {image_id_str} for request {:x}",
                    request.id
                );
                return Ok(image_id_str.clone());
            }
        }

        tracing::debug!(
            "Fetching program for request {:x} with image ID {image_id_str:?} from URI {}",
            request.id,
            request.imageUrl
        );
        let image_data = self
            .downloader
            .download(&request.imageUrl)
            .await
            .with_context(|| format!("Failed to fetch image URI: {}", request.imageUrl))?;

        let image_id = compute_image_id(&image_data)
            .with_context(|| format!("Failed to compute image ID for request {:x}", request.id))?;

        if let Some(ref image_id_str) = image_id_str {
            let required_image_id = Risc0Digest::from_hex(image_id_str)?;
            anyhow::ensure!(
                image_id == required_image_id,
                "image ID does not match requirements; expect {}, got {}",
                required_image_id,
                image_id
            );
        }

        let image_id_str = image_id.to_string();

        tracing::debug!(
            "Uploading program for request {:x} with image ID {image_id_str} to prover",
            request.id
        );
        self.prover
            .upload_image(&image_id_str, image_data)
            .await
            .context("Failed to upload image to prover")?;

        Ok(image_id_str)
    }

    async fn upload_order_input(&self, request: &crate::ProofRequest) -> Result<String> {
        Ok(match request.input.inputType {
            RequestInputType::Inline => self
                .prover
                .upload_input(
                    GuestEnv::decode(&request.input.data)
                        .with_context(|| "Failed to decode input")?
                        .stdin,
                )
                .await
                .context("Failed to upload input data")?,

            RequestInputType::Url => {
                let input_uri_str =
                    std::str::from_utf8(&request.input.data).context("input url is not utf8")?;
                tracing::debug!("Input URI string: {input_uri_str}");

                let client_addr = request.client_address();
                let input = if self.priority_requestors.is_priority_requestor(&client_addr) {
                    self.downloader.download_with_limit(input_uri_str, usize::MAX).await
                } else {
                    self.downloader.download(input_uri_str).await
                }
                .with_context(|| format!("Failed to fetch input URI: {input_uri_str}"))?;
                let input_data = GuestEnv::decode(&input)
                    .with_context(|| format!("Failed to decode input from URI: {input_uri_str}"))?
                    .stdin;

                self.prover.upload_input(input_data).await.context("Failed to upload input")?
            }
            _ => anyhow::bail!("Invalid input type: {:?}", request.input.inputType),
        })
    }

    async fn start_order(&self, order: &ProcessOrder) -> Result<String> {
        let order_id = &order.order_id;

        tracing::info!("Proving order {order_id}");

        let image_id = match order.image_id.as_ref() {
            Some(val) => val.clone(),
            None => self
                .upload_order_image(&order.request)
                .await
                .with_context(|| format!("Failed to upload image for order {order_id}"))?,
        };

        let input_id = match order.input_id.as_ref() {
            Some(val) => val.clone(),
            None => self
                .upload_order_input(&order.request)
                .await
                .with_context(|| format!("Failed to upload input for order {order_id}"))?,
        };

        let proof_id = self
            .prover
            .prove_stark(&image_id, &input_id, /* TODO assumptions */ vec![])
            .await
            .with_context(|| format!("Failed to prove customer proof STARK order {order_id}"))?;

        tracing::debug!("Order {order_id} being proved, proof id: {proof_id}");
        Ok(proof_id)
    }

    async fn compress_order_proof(
        &self,
        order_id: &str,
        stark_proof_id: &str,
        compression_type: CompressionType,
    ) -> Result<String> {
        let proof_id = match compression_type {
            CompressionType::Groth16 => self
                .snark_prover
                .compress(stark_proof_id)
                .await
                .with_context(|| format!("Failed to compress proof for order {order_id}"))?,
            CompressionType::Blake3Groth16 => {
                self.snark_prover.compress_blake3_groth16(stark_proof_id).await.with_context(
                    || format!("Failed to compress blake3 groth16 proof for order {order_id}"),
                )?
            }
            CompressionType::None => unreachable!("compression type should not be None"),
        };

        match compression_type {
            CompressionType::Groth16 => {
                tracing::trace!(
                    "Verifying compressed Groth16 receipt locally for proof_id: {proof_id}, order {order_id}"
                );
                provers::verify_groth16_receipt(&self.snark_prover, &proof_id).await?;
            }
            CompressionType::Blake3Groth16 => {
                tracing::trace!(
                    "Verifying compressed Blake3 Groth16 receipt locally for proof_id: {proof_id}, order {order_id}"
                );
                provers::verify_blake3_groth16_receipt(&self.snark_prover, &proof_id).await?;
            }
            CompressionType::None => unreachable!("compression type should not be None"),
        }

        Ok(proof_id)
    }
}

#[cfg(test)]
fn supports_risc0_selector(selector: FixedBytes<4>) -> bool {
    selector == boundless_market::contracts::UNSPECIFIED_SELECTOR
        || is_groth16_selector(selector)
        || is_blake3_groth16_selector(selector)
}

/// Classifies a verifier selector into the proof type the RISC0 backend produces for it, or
/// `None` if this backend does not serve the selector.
///
/// [`super::router::BackendRouter::register_backend`] rejects any registered selector this
/// returns `None` for, so every selector in [`Risc0Backend::selectors_for`] must classify.
fn proof_type_for_selector(selector: FixedBytes<4>) -> Option<ProofType> {
    match selector {
        UNSPECIFIED_SELECTOR | SELECTOR_FAKE_RECEIPT => Some(ProofType::Any),
        SELECTOR_GROTH16_V3_0 => Some(ProofType::Groth16),
        SELECTOR_BLAKE3_GROTH16_V0_1 | SELECTOR_FAKE_BLAKE3_GROTH16 => {
            Some(ProofType::Blake3Groth16)
        }
        _ => None,
    }
}

fn submission_path_for_risc0_selector(selector: FixedBytes<4>) -> SubmissionPath {
    match selector {
        SELECTOR_GROTH16_V3_0
        | SELECTOR_BLAKE3_GROTH16_V0_1
        | SELECTOR_FAKE_RECEIPT
        | SELECTOR_FAKE_BLAKE3_GROTH16 => SubmissionPath::Direct,
        _ => SubmissionPath::Batched,
    }
}

/// Proof compression required by a RISC0 verifier selector.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CompressionType {
    None,
    Groth16,
    Blake3Groth16,
}

/// Classifies a set-verifier root-submission error for the submitter's retry/alarm policy.
///
/// [`SetVerifierService::submit_merkle_root`] reports a confirmation timeout with a
/// `failed to confirm tx` context. The match is against the full cause chain (`{:#}`) so
/// that added context cannot hide the keyword.
fn classify_verifier_update_error(err: anyhow::Error) -> VerifierUpdateError {
    if format!("{err:#}").contains("failed to confirm tx") {
        VerifierUpdateError::TxnConfirmation(err)
    } else {
        VerifierUpdateError::Other(err)
    }
}

fn compression_type_for_selector(selector: FixedBytes<4>) -> CompressionType {
    match selector {
        SELECTOR_GROTH16_V3_0 | SELECTOR_FAKE_RECEIPT => CompressionType::Groth16,
        SELECTOR_BLAKE3_GROTH16_V0_1 | SELECTOR_FAKE_BLAKE3_GROTH16 => {
            CompressionType::Blake3Groth16
        }
        _ => CompressionType::None,
    }
}

#[async_trait]
impl Backend for Risc0Backend {
    fn id(&self) -> &BackendId {
        &self.id
    }

    fn supported_selectors(&self) -> Vec<FixedBytes<4>> {
        Self::selectors()
    }

    fn proof_type(&self, selector: FixedBytes<4>) -> Option<ProofType> {
        proof_type_for_selector(selector)
    }

    async fn evaluate_request(
        &self,
        request: EvaluationRequest,
        limits: EvaluationLimits,
    ) -> Result<RequestEvaluation, OrderPricingError> {
        self.evaluator.evaluate_request(request, limits).await
    }

    async fn process_order(&self, cmd: ProcessOrder) -> Result<OrderProcessProgress> {
        let order_id = cmd.order_id.clone();

        let mut state = match cmd.backend_state.as_ref() {
            Some(raw) => Risc0OrderState::decode(raw)?,
            None => {
                let proof_id = self.start_order(&cmd).await?;
                return Ok(OrderProcessProgress::InProgress {
                    state: Risc0OrderState { proof_id, ..Default::default() }.encode(),
                });
            }
        };

        let proof_res = self
            .prover
            .wait_for_stark(&state.proof_id)
            .await
            .context("Monitoring proof (stark) failed")?;

        let compression_type = compression_type_for_selector(cmd.request.requirements.selector);
        if compression_type != CompressionType::None && state.compressed_proof_id.is_none() {
            let compressed_proof_id =
                self.compress_order_proof(&order_id, &state.proof_id, compression_type).await?;
            state.compressed_proof_id = Some(compressed_proof_id);
            return Ok(OrderProcessProgress::InProgress { state: state.encode() });
        }

        let submission_path = submission_path_for_risc0_selector(cmd.request.requirements.selector);
        let compressed = state.compressed_proof_id.is_some();

        tracing::info!(
            "Customer Proof complete for proof_id: {}, order_id: {order_id} cycles: {:?} time: {}",
            state.proof_id,
            proof_res.stats.as_ref().map(|s| s.total_cycles),
            proof_res.elapsed_time,
        );

        Ok(OrderProcessProgress::Completed(ProcessedOrder {
            backend_id: self.id.clone(),
            order_id,
            submission_path,
            compressed,
        }))
    }

    async fn cancel_order(&self, cmd: CancelOrder) -> Result<()> {
        if let Some(raw) = cmd.backend_state.as_ref() {
            let state = Risc0OrderState::decode(raw)?;
            if cmd.is_proving {
                tracing::debug!("Cancelling proof {} for order {}", state.proof_id, cmd.order_id);
                self.prover
                    .cancel_stark(&state.proof_id)
                    .await
                    .with_context(|| format!("Failed to cancel proof {}", state.proof_id))?;
            }
        }

        Ok(())
    }

    fn batch_processor(&self) -> Option<BatchProcessorObj> {
        self.batch_processor.clone()
    }

    async fn build_fulfillments(&self, cmd: FulfillmentBatch) -> Result<SubmissionPlan> {
        anyhow::ensure!(
            cmd.backend_id == self.id,
            "backend {} cannot build fulfillments for backend {}",
            self.id,
            cmd.backend_id
        );

        let backend_state =
            cmd.state.as_ref().context("Cannot submit batch with no recorded backend state")?;
        let aggregation_state = Risc0BatchState::from_backend_state(backend_state)?;
        let assessor_proof_id = aggregation_state
            .assessor_proof_id
            .as_deref()
            .context("Cannot submit batch with no assessor receipt")?;
        let set_builder_program_id = self
            .set_builder_program_id
            .context("RISC0 backend is missing set-builder program id")?;
        let set_verifier_addr =
            self.set_verifier_addr.context("RISC0 backend is missing set-verifier address")?;

        let submission = Risc0Submission::new(self.snark_prover.clone());
        let inclusion_params =
            SetInclusionReceiptVerifierParameters { image_id: set_builder_program_id };
        let groth16_proof_id = aggregation_state
            .compressed_proof_id
            .as_ref()
            .context("Cannot submit batch with no recorded Groth16 proof ID")?;
        anyhow::ensure!(
            !aggregation_state.claim_digests.is_empty(),
            "Cannot submit batch with no claim digests"
        );
        anyhow::ensure!(
            aggregation_state.guest_state.mmr.is_finalized(),
            "Cannot submit guest state that is not finalized"
        );

        let batch_root = risc0_aggregation::merkle_root(&aggregation_state.claim_digests);
        let finalized_root = aggregation_state
            .guest_state
            .mmr
            .clone()
            .finalized_root()
            .expect("invariant: finalized MMR has a root");
        anyhow::ensure!(
            finalized_root == batch_root,
            "Guest state finalized root is inconsistent with claim digests"
        );
        let verifier_update = VerifierUpdate::SubmitMerkleRoot {
            verifier: set_verifier_addr,
            root: B256::from_slice(batch_root.as_bytes()),
            seal: submission.encode_groth16_seal(groth16_proof_id.as_str()).await?.into(),
        };

        let order_states: std::collections::HashMap<String, Risc0OrderState> = cmd
            .orders
            .iter()
            .filter_map(|o| {
                o.backend_state
                    .as_ref()
                    .map(|raw| Risc0OrderState::decode(raw).map(|s| (o.order_id.clone(), s)))
            })
            .collect::<Result<_>>()?;

        let mut orders = Vec::with_capacity(cmd.orders.len());
        let mut failed_orders = Vec::new();
        for order in cmd.orders {
            let order_id = order.order_id.clone();
            let res = async {
                let state = order_states.get(&order_id).with_context(|| {
                    format!(
                        "Order {order_id} storage-class failure: missing DB row or backend state \
                         at fulfillment build time"
                    )
                })?;
                let order_img_id = Risc0Digest::from(<[u8; 32]>::from(order.program_id));
                let order_journal = self
                    .prover
                    .get_journal(&state.proof_id)
                    .await
                    .with_context(|| {
                        format!("Failed to get order journal from prover for {order_id}")
                    })?
                    .with_context(|| format!("Order proof journal missing for {order_id}"))?;
                let order_journal_digest = order_journal.digest();
                let order_claim_digest =
                    submission.claim_digest(order_img_id, order_journal_digest);

                let seal = if is_groth16_selector(order.request.requirements.selector)
                    || is_blake3_groth16_selector(order.request.requirements.selector)
                {
                    let compressed_proof_id =
                        state.compressed_proof_id.as_deref().with_context(|| {
                            format!("Order {order_id} missing compressed proof ID for submission")
                        })?;
                    submission
                        .encode_seal_for_selector(
                            order.request.requirements.selector,
                            compressed_proof_id,
                        )
                        .await
                        .with_context(|| {
                            format!("Failed to encode seal for order {}", order.order_id)
                        })?
                } else {
                    let order_claim =
                        ReceiptClaim::ok(order_img_id, MaybePruned::Pruned(order_journal_digest));
                    let order_claim_index = aggregation_state
                        .claim_digests
                        .iter()
                        .position(|claim| *claim == order_claim_digest)
                        .ok_or_else(|| {
                            anyhow::anyhow!(
                                "Failed to find order claim {order_claim:x?} for order {} request {} in aggregated claims",
                                order.order_id,
                                order.request.id
                            )
                        })?;
                    let order_path = risc0_aggregation::merkle_path(
                        &aggregation_state.claim_digests,
                        order_claim_index,
                    );
                    tracing::debug!(
                        "Merkle path for order {} : {:x?} : {order_path:x?}",
                        order.order_id,
                        order_claim_digest
                    );
                    let set_inclusion_receipt = SetInclusionReceipt::from_path_with_verifier_params(
                        order_claim,
                        order_path,
                        inclusion_params.digest(),
                    );
                    set_inclusion_receipt.abi_encode_seal().context("Failed to encode seal")?
                };

                tracing::debug!(
                    "Seal for order {} : {}",
                    order.order_id,
                    hex::encode(seal.clone())
                );

                let request_digest =
                    order.request.eip712_signing_hash(&cmd.eip712_domain.alloy_struct());
                let request_id = order.request.id;
                let predicate_type = order.request.requirements.predicate.predicateType;

                let (claim_digest, fulfillment_data, fulfillment_data_type) = match predicate_type {
                    PredicateType::ClaimDigestMatch => (
                        ClaimDigest::from(
                            <[u8; 32]>::try_from(
                                order.request.requirements.predicate.data.0.as_ref(),
                            )
                            .context("claim digest predicate has invalid length")?,
                        ),
                        vec![],
                        FulfillmentDataType::None,
                    ),
                    PredicateType::PrefixMatch | PredicateType::DigestMatch => (
                        ClaimDigest::from(order_claim_digest),
                        FulfillmentDataImageIdAndJournal {
                            imageId: <[u8; 32]>::from(order_img_id).into(),
                            journal: order_journal.into(),
                        }
                        .abi_encode(),
                        FulfillmentDataType::ImageIdAndJournal,
                    ),
                    _ => anyhow::bail!("Invalid predicate type: {predicate_type:?}"),
                };

                Ok(OrderFulfillmentArtifact {
                    order_id: order.order_id,
                    fulfillment: MarketFulfillment {
                        id: request_id,
                        requestDigest: request_digest,
                        fulfillmentData: fulfillment_data.into(),
                        fulfillmentDataType: fulfillment_data_type,
                        claimDigest: <[u8; 32]>::from(claim_digest).into(),
                        seal: seal.into(),
                    },
                })
            }
            .await;

            match res {
                Ok(artifact) => orders.push(artifact),
                Err(error) => {
                    failed_orders.push(FailedFulfillmentOrder {
                        order_id: order_id.clone(),
                        error: error
                            .context(format!("Failed to build fulfillment for order {order_id}")),
                    });
                }
            }
        }

        let assessor = submission.assessor_receipt(assessor_proof_id).await?;
        let assessor_claim = Risc0Digest::from(assessor.claim_digest);
        let assessor_claim_index = aggregation_state
            .claim_digests
            .iter()
            .position(|claim| *claim == assessor_claim)
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "Failed to find assessor claim {assessor_claim:x?} from proof {assessor_proof_id} in aggregated claims"
                )
            })?;
        let assessor_path =
            risc0_aggregation::merkle_path(&aggregation_state.claim_digests, assessor_claim_index);
        tracing::debug!("Merkle path for assessor : {:x?} : {assessor_path:x?}", assessor_claim);

        let assessor_seal = SetInclusionReceipt::from_path_with_verifier_params(
            // TODO: Set inclusion proofs, when ABI encoded, currently don't contain anything
            // derived from the claim. So instead of constructing the journal, we simply use the
            // zero digest. We should either plumb through the data for the assessor journal, or we
            // should make an explicit way to encode an inclusion proof without the claim.
            ReceiptClaim::ok(Risc0Digest::ZERO, MaybePruned::Pruned(Risc0Digest::ZERO)),
            assessor_path,
            inclusion_params.digest(),
        );
        let assessor_seal =
            assessor_seal.abi_encode_seal().context("ABI encode assessor set inclusion receipt")?;

        Ok(SubmissionPlan {
            verifier_updates: vec![verifier_update],
            failed_orders,
            orders,
            assessor: SubmissionAssessorArtifact {
                seal: assessor_seal.into(),
                selectors: assessor.selectors,
                callbacks: assessor.callbacks,
            },
        })
    }

    async fn verifier_update_applied(&self, update: &VerifierUpdate) -> Result<bool> {
        match update {
            VerifierUpdate::SubmitMerkleRoot { verifier, root, .. } => {
                let set_verifier = self.require_set_verifier(*verifier)?;
                set_verifier
                    .contains_root(*root)
                    .await
                    .context("Failed to query set verifier for merkle root")
            }
        }
    }

    async fn apply_verifier_update(
        &self,
        update: &VerifierUpdate,
    ) -> Result<(), VerifierUpdateError> {
        match update {
            VerifierUpdate::SubmitMerkleRoot { verifier, root, seal } => {
                let set_verifier =
                    self.require_set_verifier(*verifier).map_err(VerifierUpdateError::Other)?;
                set_verifier
                    .submit_merkle_root(*root, seal.clone())
                    .await
                    .map_err(classify_verifier_update_error)?;
                Ok(())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use boundless_market::contracts::UNSPECIFIED_SELECTOR;
    use boundless_market::selector::SelectorExt;

    fn selector_ext(selector: SelectorExt) -> FixedBytes<4> {
        FixedBytes::from(selector as u32)
    }

    #[test]
    fn risc0_supports_current_risc0_selectors() {
        assert!(supports_risc0_selector(UNSPECIFIED_SELECTOR));
        assert!(supports_risc0_selector(selector_ext(SelectorExt::groth16_latest())));
        assert!(supports_risc0_selector(selector_ext(SelectorExt::blake3_groth16_latest())));
        assert!(!supports_risc0_selector(FixedBytes::from([1, 2, 3, 4])));
    }

    #[test]
    fn unspecified_selector_orders_ready_for_batch() {
        assert_eq!(
            submission_path_for_risc0_selector(UNSPECIFIED_SELECTOR),
            SubmissionPath::Batched
        );
    }

    #[test]
    fn compressed_selector_orders_ready_for_submission() {
        for selector in [
            selector_ext(SelectorExt::groth16_latest()),
            selector_ext(SelectorExt::blake3_groth16_latest()),
        ] {
            assert_eq!(submission_path_for_risc0_selector(selector), SubmissionPath::Direct);
        }
    }

    #[test]
    fn every_supported_selector_is_classified() {
        // `BackendRouter::register_backend` bails on any selector the backend cannot
        // classify. This guards that invariant for both the dev and production sets, so a
        // selector added to `selectors_for` without a `proof_type_for_selector` arm fails
        // here rather than at broker startup.
        for dev_mode in [false, true] {
            for selector in Risc0Backend::selectors_for(dev_mode) {
                assert!(
                    proof_type_for_selector(selector).is_some(),
                    "selector {selector:?} (dev_mode={dev_mode}) is served but not classified"
                );
            }
        }
    }

    #[test]
    fn classification_and_routing_dispatchers_agree() {
        // Compressed-seal selectors must submit directly; non-compressed selectors must batch.
        for dev_mode in [false, true] {
            for selector in Risc0Backend::selectors_for(dev_mode) {
                let compression = compression_type_for_selector(selector);
                let submission_path = submission_path_for_risc0_selector(selector);
                let expects_direct = !matches!(compression, CompressionType::None);
                let observed_direct = matches!(submission_path, SubmissionPath::Direct);
                assert_eq!(
                    expects_direct, observed_direct,
                    "selector {selector:?} (dev_mode={dev_mode}): \
                     compression={compression:?} but submission_path={submission_path:?}",
                );
            }
        }
    }

    #[test]
    fn production_selectors_exclude_dev_fakes() {
        // The whole suite runs with RISC0_DEV_MODE=1, so without an explicit `dev_mode=false`
        // case the production selector set is never exercised.
        let prod = Risc0Backend::selectors_for(false);
        let fake_receipt = selector_ext(SelectorExt::FakeReceipt);
        let fake_blake3 = selector_ext(SelectorExt::FakeBlake3Groth16);
        assert!(prod.contains(&UNSPECIFIED_SELECTOR));
        assert!(!prod.contains(&fake_receipt), "production set must not include FakeReceipt");
        assert!(!prod.contains(&fake_blake3), "production set must not include FakeBlake3Groth16");

        // The dev set is a strict superset of the production set.
        let dev = Risc0Backend::selectors_for(true);
        for selector in &prod {
            assert!(dev.contains(selector), "dev set is missing production selector {selector:?}");
        }
        assert!(dev.contains(&fake_receipt) && dev.contains(&fake_blake3));
    }

    async fn guard_test_backend() -> Risc0Backend {
        let config = ConfigLock::default();
        let downloader = ConfigurableDownloader::new(config.clone()).await.unwrap();
        let priority_requestors = PriorityRequestors::new(config, 1);
        let prover: ProverObj = std::sync::Arc::new(provers::DefaultProver::new());
        Risc0Backend::with_provers(prover.clone(), prover, downloader, priority_requestors)
    }

    fn guard_fulfillment_batch(
        backend_id: BackendId,
        state: Option<BackendBatchState>,
    ) -> FulfillmentBatch {
        FulfillmentBatch {
            backend_id,
            state,
            eip712_domain: eip712_domain(Address::ZERO, 1),
            orders: vec![],
        }
    }

    fn decodable_backend_state(assessor_proof_id: Option<&str>) -> BackendBatchState {
        let mut value = serde_json::json!({
            "guest_state": GuestState::initial(Risc0Digest::ZERO),
            "claim_digests": Vec::<Risc0Digest>::new(),
        });
        if let Some(id) = assessor_proof_id {
            value["assessor_proof_id"] = serde_json::json!(id);
        }
        BackendBatchState(value)
    }

    // `SubmissionPlan` is intentionally not `Debug`, so `unwrap_err` is unavailable here.
    async fn expect_build_fulfillments_err(
        backend: &Risc0Backend,
        cmd: FulfillmentBatch,
    ) -> anyhow::Error {
        match backend.build_fulfillments(cmd).await {
            Ok(_) => panic!("expected build_fulfillments to fail"),
            Err(err) => err,
        }
    }

    #[tokio::test]
    async fn build_fulfillments_rejects_mismatched_backend_id() {
        let backend = guard_test_backend().await;
        let cmd = guard_fulfillment_batch(BackendId::new("other_backend"), None);
        let err = expect_build_fulfillments_err(&backend, cmd).await;
        assert!(
            err.to_string().contains("cannot build fulfillments for backend"),
            "unexpected error: {err:#}"
        );
    }

    #[tokio::test]
    async fn build_fulfillments_requires_backend_state() {
        let backend = guard_test_backend().await;
        let cmd = guard_fulfillment_batch(Risc0Backend::default_id(), None);
        let err = expect_build_fulfillments_err(&backend, cmd).await;
        assert!(err.to_string().contains("no recorded backend state"), "unexpected error: {err:#}");
    }

    #[test]
    fn risc0_order_state_rejects_future_version() {
        let raw = BackendOrderState(serde_json::json!({
            "version": u32::MAX,
            "proof_id": "abc",
        }));
        let err = match Risc0OrderState::decode(&raw) {
            Ok(_) => panic!("future schema version must be rejected"),
            Err(err) => err,
        };
        assert!(
            err.to_string().contains("newer than this broker supports"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn risc0_order_state_defaults_missing_version() {
        let raw = BackendOrderState(serde_json::json!({ "proof_id": "abc" }));
        let decoded = Risc0OrderState::decode(&raw).unwrap();
        assert_eq!(decoded.version, RISC0_ORDER_STATE_VERSION);
        assert_eq!(decoded.proof_id, "abc");
    }

    #[test]
    fn risc0_order_state_round_trips_proof_id() {
        // Guards that the raw opaque JSON `db/fuzz_db.rs` writes still decodes to a typed state.
        let raw = BackendOrderState(serde_json::json!({ "proof_id": "fuzz_proof_42" }));
        let decoded = Risc0OrderState::decode(&raw).unwrap();
        assert_eq!(decoded.proof_id, "fuzz_proof_42");
        assert_eq!(decoded.compressed_proof_id, None);

        let re_encoded = decoded.encode();
        let re_decoded = Risc0OrderState::decode(&re_encoded).unwrap();
        assert_eq!(re_decoded.proof_id, "fuzz_proof_42");
    }

    #[tokio::test]
    async fn build_fulfillments_requires_assessor_receipt() {
        let backend = guard_test_backend().await;
        let cmd = guard_fulfillment_batch(
            Risc0Backend::default_id(),
            Some(decodable_backend_state(None)),
        );
        let err = expect_build_fulfillments_err(&backend, cmd).await;
        assert!(err.to_string().contains("no assessor receipt"), "unexpected error: {err:#}");
    }
}
