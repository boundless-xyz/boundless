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

//! The Boundless CLI is a command-line interface for interacting with Boundless.

#![deny(missing_docs)]

// TODO(victor): Break up the code below into modules.

pub mod commands;
pub mod config;
pub mod config_file;
pub(crate) mod indexer_client;

// DRY helper modules
pub mod chain;
pub mod config_ext;
pub mod contracts;
pub mod display;

use alloy::{
    primitives::{Address, Bytes},
    sol_types::{SolStruct, SolValue},
};
use anyhow::{bail, Context, Result};
use blake3_groth16::Blake3Groth16Receipt;
use boundless_assessor::{AssessorInput, Fulfillment};
use broker::{
    provers::{Bonsai, DefaultProver as BrokerDefaultProver, Prover},
    utils::prune_receipt_claim_journal,
};
use risc0_aggregation::{
    merkle_path, GuestState, SetInclusionReceipt, SetInclusionReceiptVerifierParameters,
};
use risc0_ethereum_contracts::encode_seal;
use risc0_zkvm::{
    compute_image_id,
    sha::{Digest, Digestible},
    Receipt, ReceiptClaim,
};
use std::sync::Arc;

use boundless_market::{
    contracts::{
        AssessorJournal, AssessorReceipt, EIP712DomainSaltless,
        Fulfillment as BoundlessFulfillment, FulfillmentData, PredicateType, RequestInputType,
    },
    input::GuestEnv,
    selector::{is_blake3_groth16_selector, is_groth16_selector, SupportedSelectors},
    storage::fetch_url,
    ProofRequest,
};

/// Default URL for assessor image - matches broker config defaults
pub const ASSESSOR_DEFAULT_IMAGE_URL: &str =
    "https://signal-artifacts.beboundless.xyz/v3/assessor/assessor_guest.bin";

/// Default URL for set builder image - matches broker config defaults
pub const SET_BUILDER_DEFAULT_IMAGE_URL: &str =
    "https://signal-artifacts.beboundless.xyz/v2/set-builder/guest.bin";

alloy::sol!(
    #[sol(all_derives)]
    /// The fulfillment of an order.
    struct OrderFulfilled {
        /// The root of the set.
        bytes32 root;
        /// The seal of the root.
        bytes seal;
        /// The fulfillments of the order.
        BoundlessFulfillment[] fills;
        /// The fulfillment of the assessor.
        AssessorReceipt assessorReceipt;
    }
);

impl OrderFulfilled {
    /// Creates a new [OrderFulfilled],
    pub fn new(
        fills: Vec<BoundlessFulfillment>,
        root_receipt: Receipt,
        assessor_receipt: AssessorReceipt,
    ) -> Result<Self> {
        let state = GuestState::decode(&root_receipt.journal.bytes)?;
        let root = state.mmr.finalized_root().context("failed to get finalized root")?;

        let root_seal = encode_seal(&root_receipt)?;

        Ok(OrderFulfilled {
            root: <[u8; 32]>::from(root).into(),
            seal: root_seal.into(),
            fills,
            assessorReceipt: assessor_receipt,
        })
    }
}

/// Ensure prover has the specified image, downloading and uploading if needed.
///
/// Implements fallback URL pattern:
/// 1. Check if prover already has the image (returns early if yes)
/// 2. Try downloading from default URL first (signal-artifacts.beboundless.xyz)
/// 3. Fall back to contract URL if default fails or image ID mismatches
/// 4. Verify downloaded image matches expected ID
/// 5. Upload to prover
///
/// # Arguments
/// * `prover` - The prover to upload the image to
/// * `image_label` - Label for logging (e.g., "assessor", "set builder")
/// * `expected_image_id` - Expected image ID for verification
/// * `default_url` - Default URL to try first (for fallback pattern)
/// * `contract_url` - Contract URL to use as fallback
pub async fn ensure_prover_has_image(
    prover: &Arc<dyn Prover + Send + Sync>,
    image_label: &str,
    expected_image_id: Digest,
    default_url: &str,
    contract_url: &str,
) -> Result<()> {
    let image_id_str = expected_image_id.to_string();

    // Check if prover already has the image
    if prover.has_image(&image_id_str).await.map_err(|e| {
        anyhow::anyhow!("Failed to check if prover has {} image: {}", image_label, e)
    })? {
        tracing::debug!("{} image already in prover, skipping upload", image_label);
        return Ok(());
    }

    // Try default URL first, fall back to contract URL
    let image_bytes = match fetch_url(default_url).await {
        Ok(bytes) => {
            let computed_id = compute_image_id(&bytes).with_context(|| {
                format!("Failed to compute {} image ID from default URL", image_label)
            })?;

            if computed_id == expected_image_id {
                tracing::debug!("Successfully fetched {} image from default URL", image_label);
                bytes
            } else {
                tracing::warn!(
                    "{} image ID mismatch from default URL: expected {}, got {}, falling back to contract URL",
                    image_label,
                    expected_image_id,
                    computed_id
                );
                fetch_url(contract_url).await.with_context(|| {
                    format!("Failed to download {} image from contract URL", image_label)
                })?
            }
        }
        Err(e) => {
            tracing::warn!(
                "Failed to download {} image from default URL: {}, falling back to contract URL",
                image_label,
                e
            );
            fetch_url(contract_url).await.with_context(|| {
                format!("Failed to download {} image from contract URL", image_label)
            })?
        }
    };

    // Final verification
    let computed_id = compute_image_id(&image_bytes)
        .with_context(|| format!("Failed to compute {} image ID", image_label))?;

    if computed_id != expected_image_id {
        bail!(
            "{} image ID mismatch: expected {}, got {}",
            image_label,
            expected_image_id,
            computed_id
        );
    }

    // Upload to prover
    prover
        .upload_image(&image_id_str, image_bytes)
        .await
        .map_err(|e| anyhow::anyhow!("Failed to upload {} image to prover: {}", image_label, e))?;

    tracing::debug!("Successfully uploaded {} image to prover", image_label);
    Ok(())
}

/// The default prover implementation.
/// This [DefaultProver] uses the default zkVM prover.
/// The selection of the zkVM prover is based on environment variables.
///
/// The `RISC0_PROVER` environment variable, if specified, will select the
/// following [Prover] implementation:
/// * `bonsai`: [BonsaiProver] to prove on Bonsai.
/// * `local`: LocalProver to prove locally in-process. Note: this
///   requires the `prove` feature flag.
/// * `ipc`: [ExternalProver] to prove using an `r0vm` sub-process. Note: `r0vm`
///   must be installed. To specify the path to `r0vm`, use `RISC0_SERVER_PATH`.
///
/// If `RISC0_PROVER` is not specified, the following rules are used to select a
/// Orchestrates the fulfillment of proof requests by coordinating order proving,
/// set building, and assessor verification.
///
/// Uses a `Prover` trait implementation for the actual proving backend, which can be:
/// * `broker::provers::DefaultProver` for local proving
/// * `broker::provers::Bonsai` for Bento/Bonsai remote proving
pub struct OrderFulfiller {
    prover: Arc<dyn Prover + Send + Sync>,
    set_builder_image_id: Digest,
    assessor_image_id: Digest,
    address: Address,
    domain: EIP712DomainSaltless,
    supported_selectors: SupportedSelectors,
}

impl OrderFulfiller {
    /// Creates a new [OrderFulfiller].
    pub fn new(
        prover: Arc<dyn Prover + Send + Sync>,
        set_builder_image_id: Digest,
        assessor_image_id: Digest,
        address: Address,
        domain: EIP712DomainSaltless,
    ) -> Result<Self> {
        let supported_selectors =
            SupportedSelectors::default().with_set_builder_image_id(set_builder_image_id);
        Ok(Self {
            prover,
            set_builder_image_id,
            assessor_image_id,
            address,
            domain,
            supported_selectors,
        })
    }

    pub(crate) async fn initialize_from_config<P, St, R, Si>(
        prover_config: &config::ProverConfig,
        client: &boundless_market::Client<P, St, R, Si>,
    ) -> Result<Self>
    where
        P: alloy::providers::Provider<alloy::network::Ethereum> + Clone + 'static,
    {
        prover_config.proving_backend.configure_proving_backend_with_health_check().await?;

        let prover: Arc<dyn Prover + Send + Sync> = if prover_config
            .proving_backend
            .use_default_prover
        {
            if let Ok(url) = std::env::var("BONSAI_API_URL") {
                tracing::info!("Default prover selected but BONSAI_API_URL is set, using Bonsai");
                Arc::new(Bonsai::new(
                    broker::config::ConfigLock::default(),
                    &url,
                    &std::env::var("BONSAI_API_KEY")
                        .clone()
                        .unwrap_or_else(|_| "v1:reserved:50".to_string()),
                )?)
            } else {
                Arc::new(BrokerDefaultProver::default())
            }
        } else {
            Arc::new(Bonsai::new(
                broker::config::ConfigLock::default(),
                &prover_config.proving_backend.bento_api_url,
                &prover_config
                    .proving_backend
                    .bento_api_key
                    .clone()
                    .unwrap_or_else(|| "v1:reserved:50".to_string()),
            )?)
        };

        Self::initialize(prover, client).await
    }

    /// Initialize an OrderFulfiller from a provided Prover instance.
    pub async fn initialize<P, St, R, Si>(
        prover: Arc<dyn Prover + Send + Sync>,
        client: &boundless_market::Client<P, St, R, Si>,
    ) -> Result<Self>
    where
        P: alloy::providers::Provider<alloy::network::Ethereum> + Clone + 'static,
    {
        let domain = client.boundless_market.eip712_domain().await?;

        let (assessor_image_id_bytes, assessor_url) = client.boundless_market.image_info().await?;
        let (set_builder_image_id_bytes, set_builder_url) =
            client.set_verifier.image_info().await?;

        let assessor_image_id = Digest::try_from(assessor_image_id_bytes.as_slice())?;
        let set_builder_image_id = Digest::try_from(set_builder_image_id_bytes.as_slice())?;

        tracing::debug!("Fetching Assessor program (ID: {})", assessor_image_id);
        ensure_prover_has_image(
            &prover,
            "assessor",
            assessor_image_id,
            ASSESSOR_DEFAULT_IMAGE_URL,
            &assessor_url,
        )
        .await?;

        tracing::debug!("Fetching SetBuilder program (ID: {})", set_builder_image_id);
        ensure_prover_has_image(
            &prover,
            "set builder",
            set_builder_image_id,
            SET_BUILDER_DEFAULT_IMAGE_URL,
            &set_builder_url,
        )
        .await?;

        OrderFulfiller::new(
            prover,
            set_builder_image_id,
            assessor_image_id,
            client.boundless_market.caller(),
            domain,
        )
    }

    // Proves using the configured [Prover] with the given [image_id], [input], and [assumptions].
    pub(crate) async fn prove_stark(
        &self,
        image_id: &str,
        input: Vec<u8>,
        assumption_ids: Vec<String>,
    ) -> Result<String> {
        let input_id = self
            .prover
            .upload_input(input)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to upload input: {}", e))?;

        let proof_id = self
            .prover
            .prove_stark(image_id, &input_id, assumption_ids)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to start proof: {}", e))?;

        let _result = self
            .prover
            .wait_for_stark(&proof_id)
            .await
            .map_err(|e| anyhow::anyhow!("Proof failed: {}", e))?;

        Ok(proof_id)
    }

    // Finalizes the set builder.
    pub(crate) async fn finalize(
        &self,
        claims: Vec<ReceiptClaim>,
        assumption_ids: Vec<String>,
    ) -> Result<String> {
        let input = GuestState::initial(self.set_builder_image_id)
            .into_input(claims, true)
            .context("Failed to build set builder input")?;
        let encoded_input = bytemuck::pod_collect_to_vec(&risc0_zkvm::serde::to_vec(&input)?);

        let image_id_str = self.set_builder_image_id.to_string();
        let stark_id = self.prove_stark(&image_id_str, encoded_input, assumption_ids).await?;

        // In finalize, compress to groth16
        tracing::debug!("Compressing finalized proof");
        Ok(self.prover.compress(&stark_id).await?)
    }

    // Proves the assessor.
    pub(crate) async fn assessor(
        &self,
        fills: Vec<Fulfillment>,
        assumption_ids: Vec<String>,
    ) -> Result<String> {
        let assessor_input =
            AssessorInput { domain: self.domain.clone(), fills, prover_address: self.address };

        let stdin = GuestEnv::builder().write_frame(&assessor_input.encode()).stdin;

        let image_id_str = self.assessor_image_id.to_string();
        self.prove_stark(&image_id_str, stdin, assumption_ids).await
    }

    /// Fulfills a list of orders, returning the relevant data:
    /// * A list of [Fulfillment] of the orders.
    /// * The [Receipt] of the root set.
    /// * The [SetInclusionReceipt] of the assessor.
    pub async fn fulfill(
        &self,
        orders: &[(ProofRequest, Bytes)],
    ) -> Result<(Vec<BoundlessFulfillment>, Receipt, AssessorReceipt)> {
        tracing::debug!("Fulfilling {} orders", orders.len());
        let orders_jobs = orders.iter().cloned().enumerate().map(move |(idx, (req, sig))| {
            let prover = self.prover.clone();
            let supported_selectors = self.supported_selectors.clone();
            async move {
                // Fetch and upload order image
                let order_program = fetch_url(&req.imageUrl).await?;
                let order_image_id = compute_image_id(&order_program)?;
                let order_image_id_str = order_image_id.to_string();

                // Upload to prover if not already there
                if !prover
                    .has_image(&order_image_id_str)
                    .await
                    .map_err(|e| anyhow::anyhow!("Failed to check if prover has image: {}", e))?
                {
                    prover
                        .upload_image(&order_image_id_str, order_program)
                        .await
                        .map_err(|e| anyhow::anyhow!("Failed to upload order image: {}", e))?;
                }

                let order_input: Vec<u8> = match req.input.inputType {
                    RequestInputType::Inline => GuestEnv::decode(&req.input.data)?.stdin,
                    RequestInputType::Url => {
                        GuestEnv::decode(
                            &fetch_url(
                                std::str::from_utf8(&req.input.data)
                                    .context("input url is not utf8")?,
                            )
                            .await?,
                        )?
                        .stdin
                    }
                    _ => bail!("Unsupported input type"),
                };

                let selector = req.requirements.selector;
                if !supported_selectors.is_supported(selector) {
                    bail!("Unsupported selector {}", req.requirements.selector);
                };

                tracing::debug!("Proving order 0x{:x}", req.id);
                let proof_id =
                    self.prove_stark(&order_image_id_str, order_input.clone(), vec![]).await?;
                let order_receipt = self
                    .prover
                    .get_receipt(&proof_id)
                    .await?
                    .ok_or_else(|| anyhow::anyhow!("Order receipt not found"))?;

                let order_journal = order_receipt.journal.bytes.clone();
                let order_claim = prune_receipt_claim_journal(ReceiptClaim::ok(
                    order_image_id,
                    order_journal.clone(),
                ));
                let order_claim_digest = order_claim.digest();

                let fulfillment_data = match req.requirements.predicate.predicateType {
                    PredicateType::ClaimDigestMatch => FulfillmentData::None,
                    PredicateType::PrefixMatch | PredicateType::DigestMatch => {
                        FulfillmentData::from_image_id_and_journal(order_image_id, order_journal)
                    }
                    _ => bail!("Invalid predicate type"),
                };
                let fill =
                    Fulfillment { request: req.clone(), signature: sig.into(), fulfillment_data };

                Ok::<_, anyhow::Error>((idx, proof_id, order_claim, order_claim_digest, fill))
            }
        });

        let results = futures::future::join_all(orders_jobs).await;
        let mut proof_ids = Vec::new();
        let mut claims = Vec::new();
        let mut claim_digests = Vec::new();
        let mut fills = Vec::new();
        let mut successful_indices = Vec::new();

        for result in results {
            match result {
                Err(e) => {
                    tracing::warn!("Failed to prove request: {}", e);
                    continue;
                }
                Ok((idx, receipt, claim, claim_digest, fill)) => {
                    successful_indices.push(idx);
                    proof_ids.push(receipt);
                    claims.push(claim);
                    claim_digests.push(claim_digest);
                    fills.push(fill);
                }
            }
        }

        tracing::debug!("Proving assessor");
        let assessor_receipt_id = self.assessor(fills.clone(), proof_ids.clone()).await?;
        let assessor_receipt = self
            .prover
            .get_receipt(&assessor_receipt_id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("Assessor receipt not found"))?;
        let assessor_journal = assessor_receipt.journal.bytes.clone();
        let assessor_claim = prune_receipt_claim_journal(ReceiptClaim::ok(
            self.assessor_image_id,
            assessor_journal.clone(),
        ));
        let assessor_receipt_journal: AssessorJournal =
            AssessorJournal::abi_decode(&assessor_journal)?;

        claims.push(assessor_claim.clone());
        claim_digests.push(assessor_claim.digest());

        tracing::debug!("Finalizing");
        let root_receipt_id = self
            .finalize(
                claims.clone(),
                [proof_ids.as_slice(), std::slice::from_ref(&assessor_receipt_id)].concat(),
            )
            .await?;
        let compressed_receipt_bytes = self
            .prover
            .get_compressed_receipt(&root_receipt_id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("Root receipt not found"))?;
        let root_receipt = bincode::deserialize(&compressed_receipt_bytes)
            .context("Failed to deserialize compressed finalized receipt")?;

        let verifier_parameters =
            SetInclusionReceiptVerifierParameters { image_id: self.set_builder_image_id };

        let mut boundless_fills = Vec::new();

        for (i, &order_idx) in successful_indices.iter().enumerate() {
            let order_inclusion_receipt = SetInclusionReceipt::from_path_with_verifier_params(
                claims[i].clone(),
                merkle_path(&claim_digests, i),
                verifier_parameters.digest(),
            );
            let (req, _sig) = &orders[order_idx];
            let order_seal = if is_groth16_selector(req.requirements.selector) {
                let compressed_proof_id = self
                    .prover
                    .compress(&proof_ids[i])
                    .await
                    .map_err(|e| anyhow::anyhow!("Failed to compress order proof: {}", e))?;

                let compressed_receipt_bytes = self
                    .prover
                    .get_compressed_receipt(&compressed_proof_id)
                    .await
                    .map_err(|e| anyhow::anyhow!("Failed to get compressed order receipt: {}", e))?
                    .ok_or_else(|| anyhow::anyhow!("Compressed order receipt not found"))?;

                let compressed_receipt: Receipt =
                    bincode::deserialize(&compressed_receipt_bytes)
                        .context("Failed to deserialize compressed order receipt")?;

                encode_seal(&compressed_receipt)?
            } else if is_blake3_groth16_selector(req.requirements.selector) {
                let compressed_proof_id = self
                    .prover
                    .compress_blake3_groth16(&proof_ids[i])
                    .await
                    .map_err(|e| anyhow::anyhow!("Failed to compress order proof: {}", e))?;

                let compressed_receipt_bytes = self
                    .prover
                    .get_blake3_groth16_receipt(&compressed_proof_id)
                    .await
                    .map_err(|e| anyhow::anyhow!("Failed to get compressed order receipt: {}", e))?
                    .ok_or_else(|| anyhow::anyhow!("Compressed order receipt not found"))?;

                let blake3_receipt: Blake3Groth16Receipt =
                    bincode::deserialize(&compressed_receipt_bytes)
                        .context("Failed to deserialize Blake3 Groth16 receipt")?;

                encode_seal(&blake3_receipt.into())?
            } else {
                order_inclusion_receipt.abi_encode_seal()?
            };

            let (fulfillment_data_type, fulfillment_data) =
                fills[i].fulfillment_data.fulfillment_type_and_data();
            let claim_digest = fills[i].evaluate_requirements()?;

            let fulfillment = BoundlessFulfillment {
                claimDigest: <[u8; 32]>::from(claim_digest).into(),
                fulfillmentData: fulfillment_data.into(),
                fulfillmentDataType: fulfillment_data_type,
                id: req.id,
                requestDigest: req.eip712_signing_hash(&self.domain.alloy_struct()),
                seal: order_seal.into(),
            };

            boundless_fills.push(fulfillment);
        }

        let assessor_inclusion_receipt = SetInclusionReceipt::from_path_with_verifier_params(
            assessor_claim,
            merkle_path(&claim_digests, claim_digests.len() - 1),
            verifier_parameters.digest(),
        );

        let assessor_receipt = AssessorReceipt {
            seal: assessor_inclusion_receipt.abi_encode_seal()?.into(),
            prover: self.address,
            selectors: assessor_receipt_journal.selectors,
            callbacks: assessor_receipt_journal.callbacks,
        };

        Ok((boundless_fills, root_receipt, assessor_receipt))
    }
}

#[cfg(test)]
#[allow(missing_docs)]
#[path = "../tests/common/mod.rs"]
pub mod test_common;

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::{
        node_bindings::Anvil,
        primitives::{FixedBytes, Signature, U256},
        signers::local::PrivateKeySigner,
    };
    use blake3_groth16::Blake3Groth16ReceiptClaim;
    use boundless_market::{
        contracts::{
            eip712_domain, Offer, Predicate, ProofRequest, RequestId, RequestInput, Requirements,
            UNSPECIFIED_SELECTOR,
        },
        selector::SelectorExt,
    };
    use boundless_test_utils::guests::{ECHO_ID, ECHO_PATH};
    use boundless_test_utils::market::create_test_ctx;
    use std::sync::Arc;

    async fn setup_proving_request_and_signature(
        signer: &PrivateKeySigner,
        selector: Option<SelectorExt>,
    ) -> (ProofRequest, Signature) {
        let request = ProofRequest::new(
            RequestId::new(signer.address(), 0),
            Requirements::new(Predicate::prefix_match(Digest::from(ECHO_ID), vec![1]))
                .with_selector(match selector {
                    Some(selector) => FixedBytes::from(selector as u32),
                    None => UNSPECIFIED_SELECTOR,
                }),
            format!("file://{ECHO_PATH}"),
            RequestInput::builder().write_slice(&[1, 2, 3, 4]).build_inline().unwrap(),
            Offer::default()
                .with_timeout(60)
                .with_lock_timeout(30)
                .with_max_price(U256::from(1000))
                .with_ramp_up_start(10),
        );

        let signature = request.sign_request(signer, Address::ZERO, 1).await.unwrap();
        (request, signature)
    }

    #[tokio::test]
    #[ignore = "runs a proof; slow without RISC0_DEV_MODE=1"]
    async fn test_fulfill_with_selector() {
        let anvil = Anvil::new().spawn();
        let ctx = create_test_ctx(&anvil).await.unwrap();
        let client =
            boundless_market::Client::new(ctx.customer_market.clone(), ctx.set_verifier.clone());
        let signer = PrivateKeySigner::random();
        let (request, signature) =
            setup_proving_request_and_signature(&signer, Some(SelectorExt::groth16_latest())).await;
        let prover: Arc<dyn Prover + Send + Sync> = Arc::new(BrokerDefaultProver::default());
        let mut fulfiller = OrderFulfiller::initialize(prover, &client).await.unwrap();
        fulfiller.domain = eip712_domain(Address::ZERO, 1);

        fulfiller.fulfill(&[(request, signature.as_bytes().into())]).await.unwrap();
    }

    #[tokio::test]
    #[ignore = "runs a proof; slow without RISC0_DEV_MODE=1"]
    async fn test_fulfill() {
        let anvil = Anvil::new().spawn();
        let ctx = create_test_ctx(&anvil).await.unwrap();
        let client =
            boundless_market::Client::new(ctx.customer_market.clone(), ctx.set_verifier.clone());
        let signer = PrivateKeySigner::random();
        let (request, signature) = setup_proving_request_and_signature(&signer, None).await;
        let prover: Arc<dyn Prover + Send + Sync> = Arc::new(BrokerDefaultProver::default());
        let mut fulfiller = OrderFulfiller::initialize(prover, &client).await.unwrap();
        fulfiller.domain = eip712_domain(Address::ZERO, 1);

        fulfiller.fulfill(&[(request, signature.as_bytes().into())]).await.unwrap();
    }

    #[tokio::test]
    #[ignore = "runs a proof; slow without RISC0_DEV_MODE=1"]
    async fn test_fulfill_blake3_groth16_selector() {
        let anvil = Anvil::new().spawn();
        let ctx = create_test_ctx(&anvil).await.unwrap();
        let client =
            boundless_market::Client::new(ctx.customer_market.clone(), ctx.set_verifier.clone());

        let input = [255u8; 32].to_vec(); // Example output data
        let blake3_claim_digest =
            Blake3Groth16ReceiptClaim::ok(Digest::from(ECHO_ID), input.clone()).digest();
        let signer = PrivateKeySigner::random();
        let request = ProofRequest::new(
            RequestId::new(signer.address(), 0),
            Requirements::new(Predicate::claim_digest_match(blake3_claim_digest))
                .with_selector(FixedBytes::from(SelectorExt::blake3_groth16_latest() as u32)),
            format!("file://{ECHO_PATH}"),
            RequestInput::builder().write_slice(&input).build_inline().unwrap(),
            Offer::default()
                .with_timeout(60)
                .with_lock_timeout(30)
                .with_max_price(U256::from(1000))
                .with_ramp_up_start(10),
        );

        let signature = request.sign_request(&signer, Address::ZERO, 1).await.unwrap();

        let prover: Arc<dyn Prover + Send + Sync> = Arc::new(BrokerDefaultProver::default());
        let mut fulfiller = OrderFulfiller::initialize(prover, &client).await.unwrap();
        fulfiller.domain = eip712_domain(Address::ZERO, 1);

        fulfiller.fulfill(&[(request, signature.as_bytes().into())]).await.unwrap();
    }
}
