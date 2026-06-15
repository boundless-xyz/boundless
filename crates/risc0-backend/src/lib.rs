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

use alloy::signers::local::PrivateKeySigner;
use alloy::sol_types::SolValue;
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
        boundless_market::BoundlessMarketService, build_onchain_assessor_seal, eip712_domain,
        encode_seal, AssessorJournal, Fulfillment as MarketFulfillment, FulfillmentData,
        FulfillmentDataImageIdAndJournal, FulfillmentDataType, Predicate, PredicateType,
        RequestInputType, RouterRegistry, ONCHAIN_ASSESSOR_SELECTOR, R0_ASSESSOR_SELECTOR,
        UNSPECIFIED_SELECTOR,
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

pub mod provers;

/// Whether `RISC0_DEV_MODE` is enabled.
fn is_dev_mode() -> bool {
    std::env::var("RISC0_DEV_MODE")
        .is_ok_and(|value| matches!(value.to_lowercase().as_str(), "1" | "true" | "yes"))
}

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

/// Candidate assessor selectors this broker can satisfy, in descending priority. When a verifier
/// class's `requiredAssessorClass` registers more than one of these, the broker uses the first
/// that's present — preferring the native on-chain assessor (an EIP-712 signature, no STARK proof)
/// over the R0 STARK guest.
///
/// These selectors are protocol conventions the deploy must honor (see [`ONCHAIN_ASSESSOR_SELECTOR`]
/// / [`R0_ASSESSOR_SELECTOR`]); the R0 one is version-coupled to the assessor guest the broker ships.
const ASSESSOR_PRIORITY: [FixedBytes<4>; 2] = [ONCHAIN_ASSESSOR_SELECTOR, R0_ASSESSOR_SELECTOR];

use crate::provers::{BonsaiConfig, ProverObj};
use anyhow::{Context, Result};
use boundless_backend::futures_retry::retry_with_context;

/// Construction-time settings the RISC0 backend reads once at startup.
#[derive(Clone)]
pub struct Risc0BackendConfig {
    pub bonsai_r0_zkvm_ver: Option<String>,
    pub req_retry_count: u64,
    pub req_retry_sleep_ms: u64,
    pub status_poll_ms: u64,
    pub status_poll_retry_count: u64,
    pub set_builder_guest_path: Option<PathBuf>,
    pub assessor_set_guest_path: Option<PathBuf>,
    pub set_builder_default_image_url: String,
    pub assessor_default_image_url: String,
    pub txn_timeout: u64,
}

/// Live proving-retry policy: `() -> (retry_count, retry_sleep_ms)`, read per prove.
pub type ProofRetryPolicy = Arc<dyn Fn() -> (u64, u64) + Send + Sync>;

use boundless_backend::{
    Backend, BackendBatchState, BackendError, BackendId, BackendOrderState, BatchClose,
    BatchProcessor, BatchProcessorObj, BatchSizeEstimate, BatchSizeEstimateRequest, BatchUpdate,
    CancelOrder, ClaimDigest, CloseBatch, FailedFulfillmentOrder, FulfillmentBatch,
    OrderFulfillmentArtifact, OrderProcessProgress, OrderProvingData, ProcessOrder, ProcessedOrder,
    ProofId, RouterPolicy, SubmissionAssessorArtifact, SubmissionPath, SubmissionPlan, UpdateBatch,
    VerifierUpdate, VerifierUpdateError,
};

/// Serialized [`Risc0OrderState`] shape version. [`Risc0OrderState::decode`] rejects a
/// newer-than-known version. State written before versioning defaults to 1.
const RISC0_ORDER_STATE_VERSION: u32 = 1;

fn risc0_order_state_version() -> u32 {
    RISC0_ORDER_STATE_VERSION
}

/// JSON-serialized inside [`BackendOrderState`] for the RISC0 backend.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub(crate) struct Risc0OrderState {
    #[serde(default = "risc0_order_state_version")]
    pub(crate) version: u32,
    pub(crate) proof_id: String,
    #[serde(default)]
    pub(crate) compressed_proof_id: Option<String>,
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
    pub(crate) fn decode(state: &BackendOrderState) -> Result<Self> {
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

    pub(crate) fn encode(&self) -> Result<BackendOrderState> {
        Ok(BackendOrderState(
            serde_json::to_value(self).context("Failed to encode RISC0 order state")?,
        ))
    }
}

mod batch;

pub use batch::prune_receipt_claim_journal;
use batch::{Risc0BatchProcessor, Risc0BatchState, Risc0Submission};

pub struct Risc0Backend {
    id: BackendId,
    prover: ProverObj,
    snark_prover: ProverObj,
    downloader: Arc<dyn StorageDownloader>,
    priority_check: PriorityRequestorCheck,
    evaluator: Risc0Evaluator,
    set_builder_program_id: Option<Risc0Digest>,
    set_verifier_addr: Option<Address>,
    set_verifier: Option<SetVerifierService<DynProvider>>,
    batch_processor: Option<BatchProcessorObj>,
    /// Prover key for signing the on-chain assessor `FulfillmentBatchAuth`.
    prover_signer: Option<PrivateKeySigner>,
    /// Address credited/slashed for the batch; bound into the on-chain assessor signature.
    prover_addr: Option<Address>,
    /// Chain id of the deployment, for the on-chain assessor EIP-712 domain.
    chain_id: u64,
    /// Router snapshot + derived broker policy, resolved once at construction. Always present:
    /// production snapshots the on-chain registry, tests supply a fixture.
    router_policy: RouterPolicy,
}

impl Risc0Backend {
    pub fn default_id() -> BackendId {
        BackendId::new(RISC0_V3_BACKEND_ID)
    }

    /// Builds the [`RouterPolicy`] for this backend's capabilities: its producible verifier
    /// selectors (groth16 / blake3, the dev fakes, and the set-inclusion entry at
    /// `set_inclusion_selector`) and its candidate assessors ([`ASSESSOR_PRIORITY`]).
    pub fn router_policy(
        registry: RouterRegistry,
        set_inclusion_selector: FixedBytes<4>,
        dev_mode: bool,
    ) -> RouterPolicy {
        RouterPolicy::new(
            Arc::new(registry),
            Self::producible(set_inclusion_selector, dev_mode),
            ASSESSOR_PRIORITY.to_vec(),
        )
    }

    /// The router selectors this backend's policy consults — the producible verifier selectors
    /// plus the candidate assessor selectors. Use to scope the on-chain registry snapshot.
    fn selectors_of_interest(
        set_inclusion_selector: FixedBytes<4>,
        dev_mode: bool,
    ) -> Vec<FixedBytes<4>> {
        Self::producible(set_inclusion_selector, dev_mode)
            .into_iter()
            .map(|(selector, _)| selector)
            .chain(ASSESSOR_PRIORITY)
            .collect()
    }

    /// The verifier selectors this backend can produce, each paired with the selector it emits in
    /// a seal (`UNSPECIFIED_SELECTOR` denotes set-inclusion), **in descending preference**. When a
    /// requestor signs a class id that holds several of these, [`RouterPolicy`] emits the first
    /// (most-preferred) one registered in that class. In dev mode the broker produces fake
    /// receipts, so the fake selectors are listed first — they win their proof-type class over the
    /// real selectors, which the broker cannot actually produce under `RISC0_DEV_MODE`.
    fn producible(
        set_inclusion_selector: FixedBytes<4>,
        dev_mode: bool,
    ) -> Vec<(FixedBytes<4>, FixedBytes<4>)> {
        let mut producible: Vec<(FixedBytes<4>, FixedBytes<4>)> = Vec::new();
        // Dev fakes first: under RISC0_DEV_MODE the broker emits fake-receipt seals, so they are
        // the preferred (and only producible) version for their proof-type classes.
        if dev_mode {
            producible.push((SELECTOR_FAKE_RECEIPT, SELECTOR_FAKE_RECEIPT));
            producible.push((SELECTOR_FAKE_BLAKE3_GROTH16, SELECTOR_FAKE_BLAKE3_GROTH16));
        }
        producible.push((SELECTOR_GROTH16_V3_0, SELECTOR_GROTH16_V3_0));
        producible.push((SELECTOR_BLAKE3_GROTH16_V0_1, SELECTOR_BLAKE3_GROTH16_V0_1));
        producible.push((set_inclusion_selector, UNSPECIFIED_SELECTOR));
        producible
    }

    /// Production constructor: builds the prover backends from the projected config, snapshots the
    /// on-chain router registry into a [`RouterPolicy`], and — unless `listen_only` — fetches and
    /// uploads the guest images and wires the batch processor. A listen-only broker never proves or
    /// fulfills, so it skips the proving infrastructure but still resolves the registry (selector
    /// support and assessor grouping are not optional).
    #[allow(clippy::too_many_arguments)]
    pub async fn from_deployment<P>(
        config: Risc0BackendConfig,
        bonsai_api_key: Option<&str>,
        bonsai_api_url: Option<&url::Url>,
        bento_api_url: Option<&url::Url>,
        downloader: Arc<dyn StorageDownloader>,
        priority_check: PriorityRequestorCheck,
        proof_retry: ProofRetryPolicy,
        provider: &Arc<P>,
        deployment: &Deployment,
        prover_addr: Address,
        prover_signer: PrivateKeySigner,
        chain_id: u64,
        listen_only: bool,
    ) -> Result<Self>
    where
        P: Provider<Ethereum> + Clone + 'static,
    {
        let (prover, snark_prover) =
            Self::build_provers(&config, bonsai_api_key, bonsai_api_url, bento_api_url)?;

        // The set-builder image id pinned by the set-verifier contract; its verifier-parameters
        // digest prefix is the set-inclusion verifier selector.
        let set_verifier = SetVerifierService::new(
            deployment.set_verifier_address,
            DynProvider::new(provider.clone()),
            prover_addr,
        )
        .with_timeout(std::time::Duration::from_secs(config.txn_timeout));
        let (image_id, set_builder_url) =
            set_verifier.image_info().await.context("Failed to get set builder image_info")?;
        let set_builder_img_id = Risc0Digest::from_bytes(image_id.0);
        let set_inclusion_selector = FixedBytes::<4>::from_slice(
            &SetInclusionReceiptVerifierParameters { image_id: set_builder_img_id }
                .digest()
                .as_bytes()[..4],
        );

        // Snapshot the router once: one read per selector of interest plus the classes they
        // reference. Afterwards verifier-class and assessor resolution are pure in-memory lookups
        // (no per-submission RPC).
        let market = BoundlessMarketService::new_for_broker(
            deployment.boundless_market_address,
            provider.clone(),
            prover_addr,
        );
        let registry = market
            .load_router_registry(&Self::selectors_of_interest(
                set_inclusion_selector,
                is_dev_mode(),
            ))
            .await?;
        let router_policy = Self::router_policy(registry, set_inclusion_selector, is_dev_mode());
        tracing::info!("Resolved router policy: {router_policy:?}");

        let mut backend = Self::with_provers(
            prover,
            snark_prover,
            downloader,
            priority_check,
            router_policy.clone(),
        );
        backend.set_builder_program_id = Some(set_builder_img_id);
        backend.set_verifier_addr = Some(deployment.set_verifier_address);
        backend.set_verifier = Some(set_verifier);
        backend.prover_signer = Some(prover_signer);
        backend.prover_addr = Some(prover_addr);
        backend.chain_id = chain_id;

        if !listen_only {
            backend
                .fetch_and_upload_image(
                    "set builder",
                    set_builder_img_id,
                    set_builder_url,
                    config.set_builder_guest_path.clone(),
                    config.set_builder_default_image_url.clone(),
                )
                .await
                .context("uploading set builder image")?;
            let assessor_img_id = backend.fetch_and_upload_assessor_image(&config).await?;
            backend.batch_processor = Some(Arc::new(Risc0BatchProcessor::new(
                proof_retry,
                backend.snark_prover.clone(),
                set_builder_img_id,
                assessor_img_id,
                deployment.boundless_market_address,
                prover_addr,
                chain_id,
                router_policy,
            )));
        }

        Ok(backend)
    }

    /// Constructor that takes the prover backends and a router policy explicitly. Used by tests,
    /// which build the [`RouterPolicy`] from an in-memory registry fixture so they exercise the
    /// same resolution logic as production.
    pub fn with_provers(
        prover: ProverObj,
        snark_prover: ProverObj,
        downloader: Arc<dyn StorageDownloader>,
        priority_check: PriorityRequestorCheck,
        router_policy: RouterPolicy,
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
        let evaluator = Risc0Evaluator::new(prover.clone(), downloader.clone(), preflight_cache)
            .with_image_upload_cache(image_upload_cache)
            .with_priority_check(priority_check.clone());

        Self {
            id: Self::default_id(),
            prover,
            snark_prover,
            downloader,
            priority_check,
            evaluator,
            set_builder_program_id: None,
            set_verifier_addr: None,
            set_verifier: None,
            batch_processor: None,
            prover_signer: None,
            prover_addr: None,
            chain_id: 0,
            router_policy,
        }
    }

    fn build_provers(
        config: &Risc0BackendConfig,
        bonsai_api_key: Option<&str>,
        bonsai_api_url: Option<&url::Url>,
        bento_api_url: Option<&url::Url>,
    ) -> Result<(ProverObj, ProverObj)> {
        let bonsai_cfg = || BonsaiConfig {
            bonsai_r0_zkvm_ver: config.bonsai_r0_zkvm_ver.clone(),
            req_retry_count: config.req_retry_count,
            req_retry_sleep_ms: config.req_retry_sleep_ms,
            status_poll_ms: config.status_poll_ms,
            status_poll_retry_count: config.status_poll_retry_count,
        };
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
                provers::Bonsai::new(bonsai_cfg(), url.as_ref(), key)
                    .context("Failed to construct Bonsai client")?,
            );
            return Ok((Arc::clone(&prover), prover));
        }
        if let Some(url) = bento_api_url {
            tracing::info!("Configured to run with Bento backend");
            let prover: ProverObj = Arc::new(
                provers::Bonsai::new(bonsai_cfg(), url.as_ref(), "v1:reserved:1000")
                    .context("Failed to initialize Bento client")?,
            );
            let snark_prover: ProverObj = Arc::new(
                provers::Bonsai::new(bonsai_cfg(), url.as_ref(), "v1:reserved:2000")
                    .context("Failed to initialize Bento client")?,
            );
            return Ok((prover, snark_prover));
        }
        let prover: ProverObj = Arc::new(provers::DefaultProver::new());
        Ok((Arc::clone(&prover), prover))
    }

    /// Maps a requestor-signed selector to the concrete verifier selector this backend produces:
    /// a signed router verifier *class id* becomes the producible entry selector for that class,
    /// a producible entry selector becomes the selector its seal is emitted with (the
    /// set-inclusion entry → `UNSPECIFIED_SELECTOR`, root-proof entries are identity), and the
    /// default sentinel passes through unchanged.
    fn normalize_selector(&self, signed: FixedBytes<4>) -> FixedBytes<4> {
        self.router_policy.producible_selector(signed).unwrap_or(signed)
    }

    pub fn with_set_builder_program_id(mut self, set_builder_program_id: Risc0Digest) -> Self {
        self.set_builder_program_id = Some(set_builder_program_id);
        self
    }

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

    #[allow(clippy::too_many_arguments)]
    pub fn with_test_batch_processor(
        mut self,
        proof_retry: ProofRetryPolicy,
        prover: ProverObj,
        set_builder_guest_id: Risc0Digest,
        assessor_guest_id: Risc0Digest,
        market_addr: Address,
        prover_addr: Address,
        chain_id: u64,
    ) -> Self {
        let batch_processor = Arc::new(Risc0BatchProcessor::new(
            proof_retry,
            prover,
            set_builder_guest_id,
            assessor_guest_id,
            market_addr,
            prover_addr,
            chain_id,
            self.router_policy.clone(),
        ));
        self.batch_processor = Some(batch_processor);
        self
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

    async fn fetch_and_upload_assessor_image(
        &self,
        config: &Risc0BackendConfig,
    ) -> Result<Risc0Digest> {
        // The market no longer exposes the assessor image info. The backend proves whatever assessor
        // guest it is configured with, so derive the image id from the configured ELF (local guest
        // path, falling back to the default URL) and upload it under that id.
        // TODO: #1982: to handle the assessor image properly we need to handle this when we
        //  initialize the backend and its supported selectors. The metadata in the specified
        //  entry/class needs to point to the correct URL.
        let program_bytes = if let Some(path) = config.assessor_set_guest_path.clone() {
            tokio::fs::read(&path).await.with_context(|| {
                format!("Failed to read assessor guest file: {}", path.display())
            })?
        } else {
            self.download_image(&config.assessor_default_image_url, "assessor default")
                .await
                .context("Failed to download assessor image from default URL")?
        };
        let image_id =
            compute_image_id(&program_bytes).context("Failed to compute assessor image ID")?;

        if self.snark_prover.has_image(&image_id.to_string()).await? {
            tracing::debug!("Assessor image {} already uploaded, skipping pull", image_id);
            return Ok(image_id);
        }

        tracing::debug!("Uploading assessor image {} to bento", image_id);
        self.snark_prover
            .upload_image(&image_id.to_string(), program_bytes)
            .await
            .context("Failed to upload assessor image to prover")?;
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

    async fn upload_order_image(
        &self,
        request: &boundless_market::contracts::ProofRequest,
    ) -> Result<String> {
        let predicate = Predicate::try_from(request.requirements.predicate.clone())
            .with_context(|| format!("Failed to parse predicate for request {:x}", request.id))?;

        let image_id_str = predicate.image_id().map(|image_id| image_id.to_string());

        // Claim-digest predicates do not carry an image id.
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

    async fn upload_order_input(
        &self,
        request: &boundless_market::contracts::ProofRequest,
    ) -> Result<String> {
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
                let input = if (self.priority_check)(&client_addr) {
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
/// A `failed to confirm tx` anywhere in the cause chain (`{:#}`) maps to
/// [`VerifierUpdateError::TxnConfirmation`].
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
        self.router_policy.supported_selectors()
    }

    fn proof_type(&self, selector: FixedBytes<4>) -> Option<ProofType> {
        proof_type_for_selector(self.normalize_selector(selector))
    }

    fn assessor_group(&self, selector: FixedBytes<4>) -> Result<Option<FixedBytes<4>>> {
        let group =
            self.router_policy.assessor_selector_for_signed(selector).with_context(|| {
                format!(
                    "signed verifier selector {selector} resolves to no supported assessor in the \
                 router registry"
                )
            })?;
        Ok(Some(group))
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
                    state: Risc0OrderState { proof_id, ..Default::default() }.encode()?,
                });
            }
        };

        let proof_res = self
            .prover
            .wait_for_stark(&state.proof_id)
            .await
            .context("Monitoring proof (stark) failed")?;

        // A requestor may sign a verifier *class id* rather than a specific entry selector; map it
        // to the concrete selector this backend produces in that class before deciding compression
        // / submission path.
        let selector = self.normalize_selector(cmd.request.requirements.selector);
        let compression_type = compression_type_for_selector(selector);
        if compression_type != CompressionType::None && state.compressed_proof_id.is_none() {
            let compressed_proof_id =
                self.compress_order_proof(&order_id, &state.proof_id, compression_type).await?;
            state.compressed_proof_id = Some(compressed_proof_id);
            return Ok(OrderProcessProgress::InProgress { state: state.encode()? });
        }

        let submission_path = submission_path_for_risc0_selector(selector);
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
        // Only the R0 guest path needs an aggregated assessor receipt; the on-chain assessor signs
        // instead, so its absence is expected there.
        let assessor_proof_id = aggregation_state.assessor_proof_id.as_deref();
        let set_builder_program_id = self
            .set_builder_program_id
            .context("RISC0 backend is missing set-builder program id")?;
        let set_verifier_addr =
            self.set_verifier_addr.context("RISC0 backend is missing set-verifier address")?;

        let submission = Risc0Submission::new(self.snark_prover.clone());
        let inclusion_params =
            SetInclusionReceiptVerifierParameters { image_id: set_builder_program_id };
        // The set-builder merkle root is only submitted when the batch aggregated something. An
        // all-direct-submit batch under the on-chain assessor has no aggregated root (each fill
        // carries its own groth16 seal), so there is no root to submit.
        let verifier_updates =
            if let Some(groth16_proof_id) = aggregation_state.compressed_proof_id.as_ref() {
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
                vec![VerifierUpdate::SubmitMerkleRoot {
                    verifier: set_verifier_addr,
                    root: B256::from_slice(batch_root.as_bytes()),
                    seal: submission.encode_groth16_seal(groth16_proof_id.as_str()).await?.into(),
                }]
            } else {
                vec![]
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

        // A batch is single verifier class; capture any order's signed selector before the
        // consuming loop so the assessor selection (below) can resolve the batch's verifier class
        // even if some orders fail to build.
        let batch_signed_selector = cmd.orders.first().map(|o| o.request.requirements.selector);

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

                // Normalize a signed verifier class id to the concrete selector this backend
                // produces in that class (see `normalize_selector`).
                let selector = self.normalize_selector(order.request.requirements.selector);
                let seal = if is_groth16_selector(selector) || is_blake3_groth16_selector(selector)
                {
                    let compressed_proof_id =
                        state.compressed_proof_id.as_deref().with_context(|| {
                            format!("Order {order_id} missing compressed proof ID for submission")
                        })?;
                    submission
                        .encode_seal_for_selector(selector, compressed_proof_id)
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

                tracing::debug!("Seal for order {} : {}", order.order_id, hex::encode(&seal));

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
                        ClaimDigest::from_native(order_claim_digest),
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
                    request: order.request,
                    fulfillment: MarketFulfillment {
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

        // Per-batch assessor selection. The batch shares one assessor group, so resolve it from the
        // captured signed selector.
        let signed = batch_signed_selector.context("cannot select assessor for an empty batch")?;
        let assessor_selector =
            self.router_policy.assessor_selector_for_signed(signed).with_context(|| {
                format!("signed verifier selector {signed} resolves to no supported assessor")
            })?;

        // Assessor seal. The on-chain assessor signs an EIP-712 `FulfillmentBatchAuth` over the
        // batch (no guest proof); any other selected assessor is the R0 STARK guest, which submits
        // a set-inclusion proof of the aggregated assessor receipt. Selectors/callbacks are derived
        // on-chain from the client-signed SlimRequest, so they are unused downstream and left empty
        // for on-chain.
        let assessor = if assessor_selector == ONCHAIN_ASSESSOR_SELECTOR {
            let signer = self
                .prover_signer
                .as_ref()
                .context("on-chain assessor selected but no prover signer configured")?;
            let prover = self
                .prover_addr
                .context("on-chain assessor selected but no prover address configured")?;
            let address = self
                .router_policy
                .entry(assessor_selector)
                .map(|entry| entry.implementation)
                .context("on-chain assessor selected but its adapter address is unknown")?;
            let requests: Vec<_> = orders.iter().map(|o| o.request.clone()).collect();
            let fulfillments: Vec<_> = orders.iter().map(|o| o.fulfillment.clone()).collect();
            let seal = build_onchain_assessor_seal(
                signer,
                assessor_selector,
                address,
                self.chain_id,
                &cmd.eip712_domain.alloy_struct(),
                prover,
                &requests,
                &fulfillments,
            )
            .await
            .context("Failed to build on-chain assessor seal")?;
            SubmissionAssessorArtifact { seal, selectors: vec![], callbacks: vec![] }
        } else {
            let assessor_proof_id =
                assessor_proof_id.context("Cannot submit batch with no assessor receipt")?;
            let assessor = submission.assessor_receipt(assessor_proof_id).await?;
            let assessor_claim: Risc0Digest = assessor.claim_digest.to_native();
            let assessor_claim_index = aggregation_state
                    .claim_digests
                    .iter()
                    .position(|claim| *claim == assessor_claim)
                    .ok_or_else(|| {
                        anyhow::anyhow!(
                            "Failed to find assessor claim {assessor_claim:x?} from proof {assessor_proof_id} in aggregated claims"
                        )
                    })?;
            let assessor_path = risc0_aggregation::merkle_path(
                &aggregation_state.claim_digests,
                assessor_claim_index,
            );
            tracing::debug!(
                "Merkle path for assessor : {:x?} : {assessor_path:x?}",
                assessor_claim
            );

            let inner_assessor_seal = SetInclusionReceipt::from_path_with_verifier_params(
                // TODO: Set inclusion proofs, when ABI encoded, currently don't contain anything
                // derived from the claim. So instead of constructing the journal, we simply use the
                // zero digest. We should either plumb through the data for the assessor journal, or we
                // should make an explicit way to encode an inclusion proof without the claim.
                ReceiptClaim::ok(Risc0Digest::ZERO, MaybePruned::Pruned(Risc0Digest::ZERO)),
                assessor_path,
                inclusion_params.digest(),
            );
            let inner_assessor_seal = inner_assessor_seal
                .abi_encode_seal()
                .context("ABI encode assessor set inclusion receipt")?;
            // The assessor seal is `router assessor selector ++ inner seal`.
            let seal =
                boundless_market::contracts::assessor_seal(assessor_selector, inner_assessor_seal);
            SubmissionAssessorArtifact {
                seal,
                selectors: assessor.selectors,
                callbacks: assessor.callbacks,
            }
        };

        Ok(SubmissionPlan { verifier_updates, failed_orders, orders, assessor })
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

    /// The static selector table the backend can produce root or aggregated seals for.
    /// `dev_mode` adds the dev-only fake-proof selectors; the guest-version-derived
    /// set-inclusion selector is deliberately absent (it is supported via the router policy's
    /// producible set, not this constant table) — these tests only assert the constant
    /// selectors classify and route consistently.
    fn static_selectors_for(dev_mode: bool) -> Vec<FixedBytes<4>> {
        let mut selectors =
            vec![UNSPECIFIED_SELECTOR, SELECTOR_GROTH16_V3_0, SELECTOR_BLAKE3_GROTH16_V0_1];
        if dev_mode {
            selectors.push(SELECTOR_FAKE_RECEIPT);
            selectors.push(SELECTOR_FAKE_BLAKE3_GROTH16);
        }
        selectors
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
        // Every selector the backend serves must classify, for both the dev and production sets.
        for dev_mode in [false, true] {
            for selector in static_selectors_for(dev_mode) {
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
            for selector in static_selectors_for(dev_mode) {
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
        let prod = static_selectors_for(false);
        let fake_receipt = selector_ext(SelectorExt::FakeReceipt);
        let fake_blake3 = selector_ext(SelectorExt::FakeBlake3Groth16);
        assert!(prod.contains(&UNSPECIFIED_SELECTOR));
        assert!(!prod.contains(&fake_receipt), "production set must not include FakeReceipt");
        assert!(!prod.contains(&fake_blake3), "production set must not include FakeBlake3Groth16");

        // The dev set is a strict superset of the production set.
        let dev = static_selectors_for(true);
        for selector in &prod {
            assert!(dev.contains(selector), "dev set is missing production selector {selector:?}");
        }
        assert!(dev.contains(&fake_receipt) && dev.contains(&fake_blake3));
    }

    /// With both assessors registered in the required class, the broker prefers the on-chain
    /// assessor — for the chain-default sentinel and for a specific entry selector alike (they
    /// resolve to the same verifier class).
    #[test]
    fn router_policy_prefers_onchain_assessor() {
        let policy = Risc0Backend::router_policy(
            boundless_test_utils::market::test_router_registry(Risc0Digest::ZERO, true),
            boundless_test_utils::market::set_verifier_selector(Risc0Digest::ZERO),
            true,
        );
        assert_eq!(
            policy.assessor_selector_for_signed(UNSPECIFIED_SELECTOR),
            Some(ONCHAIN_ASSESSOR_SELECTOR)
        );
        assert_eq!(
            policy.assessor_selector_for_signed(SELECTOR_GROTH16_V3_0),
            Some(ONCHAIN_ASSESSOR_SELECTOR)
        );
    }

    /// Without an on-chain assessor entry, the R0 STARK guest assessor is selected.
    #[test]
    fn router_policy_falls_back_to_r0_guest_assessor() {
        let policy = Risc0Backend::router_policy(
            boundless_test_utils::market::test_router_registry(Risc0Digest::ZERO, false),
            boundless_test_utils::market::set_verifier_selector(Risc0Digest::ZERO),
            true,
        );
        assert_eq!(
            policy.assessor_selector_for_signed(UNSPECIFIED_SELECTOR),
            Some(R0_ASSESSOR_SELECTOR)
        );
    }

    /// A verifier class whose required assessor class registers none of the broker's candidate
    /// assessors is unsupported: no class is advertised and no selector resolves to an assessor.
    #[test]
    fn router_policy_requires_a_candidate_assessor() {
        use boundless_market::contracts::{RouterEntry, RouterRegistry};

        let verifier_class = FixedBytes([0x00, 0x00, 0x00, 0x10]);
        let assessor_class = FixedBytes([0x00, 0x00, 0x00, 0x20]);
        let registry = RouterRegistry::from_parts(
            verifier_class,
            std::collections::HashMap::from([(
                SELECTOR_GROTH16_V3_0,
                RouterEntry {
                    implementation: Address::repeat_byte(0x01),
                    class_id: verifier_class,
                    gas_limit: 0,
                },
            )]),
            std::collections::HashMap::from([
                (verifier_class, assessor_class),
                (assessor_class, FixedBytes::ZERO),
            ]),
        );
        let policy =
            Risc0Backend::router_policy(registry, FixedBytes([0xAB, 0xCD, 0xEF, 0x01]), true);
        assert!(policy.supported_classes().is_empty());
        assert_eq!(policy.assessor_selector_for_signed(SELECTOR_GROTH16_V3_0), None);
        assert_eq!(policy.assessor_selector_for_signed(UNSPECIFIED_SELECTOR), None);
    }

    /// `supported_selectors` advertises only what the backend can actually seal: the default
    /// sentinel, every registry-covered producible entry selector with a reachable assessor, and
    /// the supported verifier class ids.
    #[tokio::test]
    async fn supported_selectors_follow_assessor_availability() {
        let backend = guard_test_backend().await;
        let selectors = backend.supported_selectors();
        assert!(selectors.contains(&UNSPECIFIED_SELECTOR));
        // Every registry-covered producible entry selector — root proofs, the dev-mode fakes
        // (the test policy is built with dev_mode), and the guest-version-derived set-inclusion
        // selector, so an exact-version pin of the default proof type is accepted.
        assert!(selectors.contains(&SELECTOR_GROTH16_V3_0));
        assert!(selectors.contains(&SELECTOR_BLAKE3_GROTH16_V0_1));
        assert!(selectors.contains(&SELECTOR_FAKE_RECEIPT));
        assert!(selectors.contains(&SELECTOR_FAKE_BLAKE3_GROTH16));
        let set_inclusion = boundless_test_utils::market::set_verifier_selector(Risc0Digest::ZERO);
        assert!(
            selectors.contains(&set_inclusion),
            "set-inclusion entry selector {set_inclusion} must be supported (the default proof path)"
        );
        // One supported class id per proof type the backend can produce in.
        assert!(selectors.contains(&boundless_test_utils::market::R0_SET_INCLUSION_CLASS_ID));
        assert!(selectors.contains(&boundless_test_utils::market::R0_GROTH16_CLASS_ID));
        assert!(selectors.contains(&boundless_test_utils::market::R0_GROTH16_BLAKE3_CLASS_ID));

        // A set-inclusion pin normalizes to the seal the backend emits for it (the
        // unspecified-selector aggregated path), so pricing classifies it as a known proof type.
        assert_eq!(backend.proof_type(set_inclusion), Some(ProofType::Any));
    }

    async fn guard_test_backend() -> Risc0Backend {
        let downloader: Arc<dyn StorageDownloader> = Arc::new(
            boundless_market::storage::StandardDownloader::from_config(Default::default()).await,
        );
        let priority_check: PriorityRequestorCheck = Arc::new(|_| false);
        let prover: ProverObj = std::sync::Arc::new(provers::DefaultProver::new());
        // A non-connecting provider: the guard tests reach `build_fulfillments` error paths before
        // any set-verifier RPC, so the URL is never dialed. Set-builder id + set-verifier address
        // are present only so those upfront checks pass and the assessor-path guards are reachable.
        let provider = Arc::new(
            alloy::providers::ProviderBuilder::new()
                .connect_http("http://localhost:8545".parse().unwrap()),
        );
        // R0-only registry fixture so resolution selects the guest-assessor path.
        let policy = Risc0Backend::router_policy(
            boundless_test_utils::market::test_router_registry(Risc0Digest::ZERO, false),
            boundless_test_utils::market::set_verifier_selector(Risc0Digest::ZERO),
            true,
        );
        Risc0Backend::with_provers(prover.clone(), prover, downloader, priority_check, policy)
            .with_set_builder_program_id(Risc0Digest::ZERO)
            .with_set_verifier(Address::ZERO, provider, Address::ZERO)
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

    // `SubmissionPlan` is not `Debug`, so `unwrap_err` is unavailable.
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
        // Raw opaque JSON decodes to a typed state.
        let raw = BackendOrderState(serde_json::json!({ "proof_id": "fuzz_proof_42" }));
        let decoded = Risc0OrderState::decode(&raw).unwrap();
        assert_eq!(decoded.proof_id, "fuzz_proof_42");
        assert_eq!(decoded.compressed_proof_id, None);

        let re_encoded = decoded.encode().unwrap();
        let re_decoded = Risc0OrderState::decode(&re_encoded).unwrap();
        assert_eq!(re_decoded.proof_id, "fuzz_proof_42");
    }

    #[tokio::test]
    async fn build_fulfillments_rejects_empty_batch() {
        let backend = guard_test_backend().await;
        let cmd = guard_fulfillment_batch(
            Risc0Backend::default_id(),
            Some(decodable_backend_state(None)),
        );
        let err = expect_build_fulfillments_err(&backend, cmd).await;
        assert!(err.to_string().contains("empty batch"), "unexpected error: {err:#}");
    }

    #[tokio::test]
    async fn build_fulfillments_requires_assessor_receipt() {
        use boundless_market::contracts::{
            Offer, ProofRequest, RequestId, RequestInput, RequestInputType, Requirements,
        };

        let backend = guard_test_backend().await;
        let mut cmd = guard_fulfillment_batch(
            Risc0Backend::default_id(),
            Some(decodable_backend_state(None)),
        );
        // One order with a bogus proof id: its per-order artifact fails (tolerated, collected in
        // `failed_orders`), but its default-sentinel selector drives the per-batch assessor
        // selection to the R0 guest path, whose missing aggregated receipt must error.
        let request = ProofRequest::new(
            RequestId::new(Address::ZERO, 1),
            Requirements::new(Predicate::prefix_match(
                Risc0Digest::ZERO,
                alloy::primitives::Bytes::default(),
            )),
            "test",
            RequestInput { inputType: RequestInputType::Inline, data: Default::default() },
            Offer {
                minPrice: U256::from(1),
                maxPrice: U256::from(10),
                rampUpStart: 0,
                timeout: 1000,
                lockTimeout: 1000,
                rampUpPeriod: 1,
                lockCollateral: U256::ZERO,
            },
        );
        cmd.orders.push(boundless_backend::FulfillmentOrder {
            order_id: "order-1".to_string(),
            request,
            program_id: [0u8; 32].into(),
            backend_state: Some(
                Risc0OrderState { proof_id: "missing".to_string(), ..Default::default() }
                    .encode()
                    .unwrap(),
            ),
        });
        let err = expect_build_fulfillments_err(&backend, cmd).await;
        assert!(err.to_string().contains("no assessor receipt"), "unexpected error: {err:#}");
    }
}
