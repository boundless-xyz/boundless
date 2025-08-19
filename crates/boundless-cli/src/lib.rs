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

//! The Boundless CLI is a command-line interface for interacting with Boundless.

#![deny(missing_docs)]

use alloy::{
    primitives::{Address, Bytes},
    sol_types::{SolStruct, SolValue},
};
use anyhow::{bail, Context, Result};
use bonsai_sdk::non_blocking::Client as BonsaiClient;
use boundless_assessor::{AssessorInput, Fulfillment};
use chrono::{DateTime, Local};
use risc0_aggregation::{
    merkle_path, GuestState, SetInclusionReceipt, SetInclusionReceiptVerifierParameters,
};
use risc0_ethereum_contracts::encode_seal;
use risc0_zkvm::{
    compute_image_id, default_prover,
    sha::{Digest, Digestible},
    ExecutorEnv, ProverOpts, Receipt, ReceiptClaim,
};

use boundless_market::{
    contracts::{
        boundless_market_contract::CallbackData, AssessorJournal, AssessorReceipt,
        EIP712DomainSaltless, Fulfillment as BoundlessFulfillment, FulfillmentClaimData,
        FulfillmentDataType, PredicateType, RequestInputType,
    },
    input::GuestEnv,
    selector::{is_groth16_selector, is_shrink_bitvm2_selector, SupportedSelectors},
    storage::fetch_url,
    ProofRequest,
};
use tempfile::tempdir;

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

/// Converts a timestamp to a [DateTime] in the local timezone.
pub fn convert_timestamp(timestamp: u64) -> DateTime<Local> {
    let t = DateTime::from_timestamp(timestamp as i64, 0).expect("invalid timestamp");
    t.with_timezone(&Local)
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
/// [Prover]:
/// * [BonsaiProver] if the `BONSAI_API_URL` and `BONSAI_API_KEY` environment
///   variables are set unless `RISC0_DEV_MODE` is enabled.
/// * LocalProver if the `prove` feature flag is enabled.
/// * [ExternalProver] otherwise.
pub struct DefaultProver {
    set_builder_program: Vec<u8>,
    set_builder_image_id: Digest,
    assessor_program: Vec<u8>,
    address: Address,
    domain: EIP712DomainSaltless,
    supported_selectors: SupportedSelectors,
}

impl DefaultProver {
    /// Creates a new [DefaultProver].
    pub fn new(
        set_builder_program: Vec<u8>,
        assessor_program: Vec<u8>,
        address: Address,
        domain: EIP712DomainSaltless,
    ) -> Result<Self> {
        let set_builder_image_id = compute_image_id(&set_builder_program)?;
        let supported_selectors =
            SupportedSelectors::default().with_set_builder_image_id(set_builder_image_id);
        Ok(Self {
            set_builder_program,
            set_builder_image_id,
            assessor_program,
            address,
            domain,
            supported_selectors,
        })
    }

    // Proves the given [program] with the given [input] and [assumptions].
    // The [opts] parameter specifies the prover options.
    pub(crate) async fn prove(
        &self,
        program: Vec<u8>,
        input: Vec<u8>,
        assumptions: Vec<Receipt>,
        opts: ProverOpts,
    ) -> Result<Receipt> {
        let receipt = tokio::task::spawn_blocking(move || {
            let mut env = ExecutorEnv::builder();
            env.write_slice(&input);
            for assumption_receipt in assumptions.iter() {
                env.add_assumption(assumption_receipt.clone());
            }
            let env = env.build()?;

            default_prover().prove_with_opts(env, &program, &opts)
        })
        .await??
        .receipt;
        Ok(receipt)
    }

    pub(crate) async fn compress(&self, succinct_receipt: &Receipt) -> Result<Receipt> {
        let prover = default_prover();
        if prover.get_name() == "bonsai" {
            return compress_with_bonsai(succinct_receipt).await;
        }
        if is_dev_mode() {
            return Ok(succinct_receipt.clone());
        }

        let receipt = succinct_receipt.clone();
        tokio::task::spawn_blocking(move || {
            default_prover().compress(&ProverOpts::groth16(), &receipt)
        })
        .await?
    }

    pub(crate) async fn shrink_bitvm2(&self, receipt: &Receipt) -> Result<Receipt> {
        if receipt.journal.bytes.len() != 32 {
            bail!(
                "Shrink BitVM2 requires a journal of 32 bytes, got {}",
                receipt.journal.bytes.len()
            );
        }
        if is_dev_mode() {
            return Ok(receipt.clone());
        }
        let succinct_receipt = receipt.inner.succinct().unwrap();
        let p254_receipt = risc0_zkvm::recursion::identity_p254(succinct_receipt)
            .context("identity predicate failed")?;
        let temp_dir = tempdir().context("Failed to crate tmpdir")?;
        let receipt = shrink_bitvm2::prove_and_verify(
            "boundless_cli",
            temp_dir.path(),
            p254_receipt,
            receipt.journal.clone(),
        )
        .await?;
        Ok(receipt)
    }

    // Finalizes the set builder.
    pub(crate) async fn finalize(
        &self,
        claims: Vec<ReceiptClaim>,
        assumptions: Vec<Receipt>,
    ) -> Result<Receipt> {
        let input = GuestState::initial(self.set_builder_image_id)
            .into_input(claims, true)
            .context("Failed to build set builder input")?;
        let encoded_input = bytemuck::pod_collect_to_vec(&risc0_zkvm::serde::to_vec(&input)?);

        self.prove(
            self.set_builder_program.clone(),
            encoded_input,
            assumptions,
            ProverOpts::groth16(),
        )
        .await
    }

    // Proves the assessor.
    pub(crate) async fn assessor(
        &self,
        fills: Vec<Fulfillment>,
        receipts: Vec<Receipt>,
    ) -> Result<Receipt> {
        let assessor_input =
            AssessorInput { domain: self.domain.clone(), fills, prover_address: self.address };

        let stdin = GuestEnv::builder().write_frame(&assessor_input.encode()).stdin;

        self.prove(self.assessor_program.clone(), stdin, receipts, ProverOpts::succinct()).await
    }

    /// Fulfills a list of orders, returning the relevant data:
    /// * A list of [Fulfillment] of the orders.
    /// * The [Receipt] of the root set.
    /// * The [SetInclusionReceipt] of the assessor.
    pub async fn fulfill(
        &self,
        orders: &[(ProofRequest, Bytes)],
    ) -> Result<(Vec<BoundlessFulfillment>, Receipt, AssessorReceipt)> {
        let orders_jobs = orders.iter().cloned().map(|(req, sig)| async move {
            let order_program = fetch_url(&req.imageUrl).await?;
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
            if !self.supported_selectors.is_supported(selector) {
                bail!("Unsupported selector {}", req.requirements.selector);
            };

            let order_receipt = self
                .prove(order_program.clone(), order_input.clone(), vec![], ProverOpts::succinct())
                .await?;

            let order_journal = order_receipt.journal.bytes.clone();
            let order_image_id = compute_image_id(&order_program)?;
            let order_claim = ReceiptClaim::ok(order_image_id, order_journal.clone());
            let order_claim_digest = order_claim.digest();

            let fulfillment_data = match req.requirements.predicate.predicateType {
                PredicateType::ClaimDigestMatch => FulfillmentClaimData::from_claim_digest(
                    req.requirements.predicate.claim_digest().unwrap(),
                ),
                _ => FulfillmentClaimData::from_image_id_and_journal(order_image_id, order_journal),
            };
            let fill =
                Fulfillment { request: req.clone(), signature: sig.into(), fulfillment_data };

            Ok::<_, anyhow::Error>((order_receipt, order_claim, order_claim_digest, fill))
        });

        let results = futures::future::join_all(orders_jobs).await;
        let mut receipts = Vec::new();
        let mut claims = Vec::new();
        let mut claim_digests = Vec::new();
        let mut fills = Vec::new();

        for (i, result) in results.into_iter().enumerate() {
            if let Err(e) = result {
                tracing::warn!("Failed to prove request 0x{:x}: {}", orders[i].0.id, e);
                continue;
            }
            let (receipt, claim, claim_digest, fill) = result?;
            receipts.push(receipt);
            claims.push(claim);
            claim_digests.push(claim_digest);
            fills.push(fill);
        }

        let assessor_receipt = self.assessor(fills.clone(), receipts.clone()).await?;
        let assessor_journal = assessor_receipt.journal.bytes.clone();
        let assessor_image_id = compute_image_id(&self.assessor_program)?;
        let assessor_claim = ReceiptClaim::ok(assessor_image_id, assessor_journal.clone());
        let assessor_receipt_journal: AssessorJournal =
            AssessorJournal::abi_decode(&assessor_journal)?;

        receipts.push(assessor_receipt);
        claims.push(assessor_claim.clone());
        claim_digests.push(assessor_claim.digest());

        let root_receipt = self.finalize(claims.clone(), receipts.clone()).await?;

        let verifier_parameters =
            SetInclusionReceiptVerifierParameters { image_id: self.set_builder_image_id };

        let mut boundless_fills = Vec::new();

        for i in 0..fills.len() {
            let order_inclusion_receipt = SetInclusionReceipt::from_path_with_verifier_params(
                claims[i].clone(),
                merkle_path(&claim_digests, i),
                verifier_parameters.digest(),
            );
            let (req, _sig) = &orders[i];
            let order_seal = if is_groth16_selector(req.requirements.selector) {
                let receipt = self.compress(&receipts[i]).await?;
                encode_seal(&receipt)?
            } else if is_shrink_bitvm2_selector(req.requirements.selector) {
                let receipt = self.shrink_bitvm2(&receipts[i]).await?;
                encode_seal(&receipt)?
            } else {
                order_inclusion_receipt.abi_encode_seal()?
            };

            // For now, we default to not providing journals with claim digest match, but you could if it is a R0 ZKVM commit digest.
            let (claim_digest, fulfillment_data, fulfillment_data_type) =
                match req.requirements.predicate.predicateType {
                    PredicateType::ClaimDigestMatch => (
                        <[u8; 32]>::from(fills[i].fulfillment_data.claim_digest().unwrap()).into(),
                        vec![],
                        FulfillmentDataType::None,
                    ),
                    PredicateType::PrefixMatch | PredicateType::DigestMatch => (
                        <[u8; 32]>::from(claims[i].digest()).into(),
                        CallbackData {
                            imageId: <[u8; 32]>::from(
                                fills[i].fulfillment_data.image_id().unwrap(),
                            )
                            .into(),
                            journal: fills[i].fulfillment_data.journal().unwrap().clone(),
                        }
                        .abi_encode(),
                        FulfillmentDataType::ImageIdAndJournal,
                    ),
                    _ => {
                        bail!("Invalid predicate type");
                    }
                };

            let fulfillment = BoundlessFulfillment {
                claimDigest: claim_digest,
                fulfillmentData: fulfillment_data.into(),
                fulfillmentDataType: fulfillment_data_type,
                id: req.id,
                requestDigest: req.eip712_signing_hash(&self.domain.alloy_struct()),
                seal: order_seal.into(),
                predicateType: req.requirements.predicate.predicateType,
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

async fn compress_with_bonsai(succinct_receipt: &Receipt) -> Result<Receipt> {
    let client = BonsaiClient::from_env(risc0_zkvm::VERSION)?;
    let encoded_receipt = bincode::serialize(succinct_receipt)?;
    let receipt_id = client.upload_receipt(encoded_receipt).await?;
    let snark_id = client.create_snark(receipt_id).await?;
    loop {
        let status = snark_id.status(&client).await?;
        match status.status.as_ref() {
            "RUNNING" => {
                tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
                continue;
            }
            "SUCCEEDED" => {
                let receipt_buf = client.download(&status.output.unwrap()).await?;
                let snark_receipt: Receipt = bincode::deserialize(&receipt_buf)?;
                return Ok(snark_receipt);
            }
            status_code => {
                let err_msg = status.error_msg.unwrap_or_default();
                return Err(anyhow::anyhow!(
                    "snark proving failed with status {status_code}: {err_msg}"
                ));
            }
        }
    }
}

// Returns `true` if the dev mode environment variable is enabled.
fn is_dev_mode() -> bool {
    std::env::var("RISC0_DEV_MODE")
        .ok()
        .map(|x| x.to_lowercase())
        .filter(|x| x == "1" || x == "true" || x == "yes")
        .is_some()
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::{
        primitives::{FixedBytes, Signature},
        signers::local::PrivateKeySigner,
    };
    use boundless_market::contracts::{
        eip712_domain, Offer, Predicate, ProofRequest, RequestId, RequestInput, Requirements,
        UNSPECIFIED_SELECTOR,
    };
    use boundless_market_test_utils::{ASSESSOR_GUEST_ELF, ECHO_ID, ECHO_PATH, SET_BUILDER_ELF};
    use risc0_ethereum_contracts::selector::Selector;
    use shrink_bitvm2::ShrinkBitvm2ReceiptClaim;

    async fn setup_proving_request_and_signature(
        signer: &PrivateKeySigner,
        selector: Option<Selector>,
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
            Offer::default(),
        );

        let signature = request.sign_request(signer, Address::ZERO, 1).await.unwrap();
        (request, signature)
    }

    #[tokio::test]
    #[ignore = "runs a proof; slow without RISC0_DEV_MODE=1"]
    async fn test_fulfill_with_selector() {
        let signer = PrivateKeySigner::random();
        let (request, signature) =
            setup_proving_request_and_signature(&signer, Some(Selector::Groth16V2_2)).await;

        let domain = eip712_domain(Address::ZERO, 1);
        let prover = DefaultProver::new(
            SET_BUILDER_ELF.to_vec(),
            ASSESSOR_GUEST_ELF.to_vec(),
            Address::ZERO,
            domain,
        )
        .expect("failed to create prover");

        prover.fulfill(&[(request, signature.as_bytes().into())]).await.unwrap();
    }

    #[tokio::test]
    #[ignore = "runs a proof; slow without RISC0_DEV_MODE=1"]
    async fn test_fulfill() {
        let signer = PrivateKeySigner::random();
        let (request, signature) = setup_proving_request_and_signature(&signer, None).await;

        let domain = eip712_domain(Address::ZERO, 1);
        let prover = DefaultProver::new(
            SET_BUILDER_ELF.to_vec(),
            ASSESSOR_GUEST_ELF.to_vec(),
            Address::ZERO,
            domain,
        )
        .expect("failed to create prover");

        prover.fulfill(&[(request, signature.as_bytes().into())]).await.unwrap();
    }
    #[tokio::test]
    #[test_log::test]
    async fn test_shrink() {
        let input = [255u8; 32].to_vec(); // Example output data
        let blake3_claim_digest =
            ShrinkBitvm2ReceiptClaim::ok(Digest::from(ECHO_ID), input.clone()).digest();
        let signer = PrivateKeySigner::random();
        let request = ProofRequest::new(
            RequestId::new(signer.address(), 0),
            Requirements::new(Predicate::claim_digest_match(blake3_claim_digest))
                .with_selector(FixedBytes::from(Selector::ShrinkBitvm2V0_1 as u32)),
            format!("file://{ECHO_PATH}"),
            RequestInput::builder().write_slice(&input).build_inline().unwrap(),
            Offer::default(),
        );

        let signature = request.sign_request(&signer, Address::ZERO, 1).await.unwrap();
        let domain = eip712_domain(Address::ZERO, 1);
        let prover = DefaultProver::new(
            SET_BUILDER_ELF.to_vec(),
            ASSESSOR_GUEST_ELF.to_vec(),
            Address::ZERO,
            domain,
        )
        .expect("failed to create prover");

        prover.fulfill(&[(request, signature.as_bytes().into())]).await.unwrap();
    }
}
