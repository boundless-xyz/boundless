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

//! The Boundless CLI is a command-line interface for interacting with the Boundless Market API.

#![deny(missing_docs)]

use alloy::{
    primitives::Address,
    sol_types::{SolStruct, SolValue},
};
use anyhow::{bail, Context, Result};
use boundless_assessor::{AssessorInput, Fulfillment};
use risc0_aggregation::{
    merkle_path, GuestInput, GuestOutput, SetInclusionReceipt,
    SetInclusionReceiptVerifierParameters,
};
use risc0_ethereum_contracts::encode_seal;
use risc0_zkvm::{
    compute_image_id, default_prover,
    sha::{Digest, Digestible},
    ExecutorEnv, ProverOpts, Receipt, ReceiptClaim,
};
use url::Url;

use boundless_market::{
    contracts::{EIP721DomainSaltless, Fulfillment as BoundlessFulfillment, InputType},
    input::InputEnv,
    order_stream_client::Order,
};

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
        /// The seal of the assessor.
        bytes assessorSeal;
        /// The prover address.
        address prover;
    }
);

impl OrderFulfilled {
    /// Creates a new [OrderFulfilled],
    pub fn new(
        fill: BoundlessFulfillment,
        root_receipt: Receipt,
        assessor_receipt: SetInclusionReceipt<ReceiptClaim>,
        prover: Address,
    ) -> Result<Self> {
        let root = <GuestOutput>::abi_decode(&root_receipt.journal.bytes, true)?.root();
        let root_seal = encode_seal(&root_receipt)?;
        let assessor_seal = assessor_receipt.abi_encode_seal()?;

        Ok(OrderFulfilled {
            root: <[u8; 32]>::from(root).into(),
            seal: root_seal.into(),
            fills: vec![fill],
            assessorSeal: assessor_seal.into(),
            prover,
        })
    }
}

/// Fetches the content of a URL.
/// Supported URL schemes are `http`, `https`, and `file`.
pub async fn fetch_url(url_str: &str) -> Result<Vec<u8>> {
    tracing::debug!("Fetching URL: {}", url_str);
    let url = Url::parse(url_str)?;

    match url.scheme() {
        "http" | "https" => fetch_http(&url).await,
        "file" => fetch_file(&url).await,
        _ => bail!("unsupported URL scheme: {}", url.scheme()),
    }
}

async fn fetch_http(url: &Url) -> Result<Vec<u8>> {
    let response = reqwest::get(url.as_str()).await?;
    let status = response.status();
    if !status.is_success() {
        bail!("HTTP request failed with status: {}", status);
    }

    Ok(response.bytes().await?.to_vec())
}

async fn fetch_file(url: &Url) -> Result<Vec<u8>> {
    let path = std::path::Path::new(url.path());
    let data = tokio::fs::read(path).await?;
    Ok(data)
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
    set_builder_elf: Vec<u8>,
    set_builder_image_id: Digest,
    assessor_elf: Vec<u8>,
    address: Address,
    domain: EIP721DomainSaltless,
}

impl DefaultProver {
    /// Creates a new [DefaultProver].
    pub fn new(
        set_builder_elf: Vec<u8>,
        assessor_elf: Vec<u8>,
        address: Address,
        domain: EIP721DomainSaltless,
    ) -> Result<Self> {
        let set_builder_image_id = compute_image_id(&set_builder_elf)?;
        Ok(Self { set_builder_elf, set_builder_image_id, assessor_elf, address, domain })
    }

    // Proves the given [elf] with the given [input] and [assumptions].
    // The [opts] parameter specifies the prover options.
    pub(crate) async fn prove(
        &self,
        elf: Vec<u8>,
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
            default_prover().prove_with_opts(env, &elf, &opts)
        })
        .await??
        .receipt;
        Ok(receipt)
    }

    // Proves the join of two sets.
    // The [left] and [right] parameters are the receipts of the sets to join.
    // TODO: Consider using a more generic approach to join sets. Here we always assume
    //       that the join is the last operation in the set builder, and so we use the
    //       [ProverOpts::groth16] options.
    pub(crate) async fn join(&self, left: Receipt, right: Receipt) -> Result<Receipt> {
        let left_output = <GuestOutput>::abi_decode(&left.journal.bytes, true)?;
        let right_output = <GuestOutput>::abi_decode(&right.journal.bytes, true)?;
        let input = GuestInput::Join {
            self_image_id: self.set_builder_image_id,
            left_set_root: left_output.root(),
            right_set_root: right_output.root(),
        };
        let encoded_input = bytemuck::pod_collect_to_vec(&risc0_zkvm::serde::to_vec(&input)?);
        self.prove(
            self.set_builder_elf.clone(),
            encoded_input,
            vec![left, right],
            ProverOpts::groth16(),
        )
        .await
    }

    // Proves a singleton set.
    pub(crate) async fn singleton(&self, receipt: Receipt) -> Result<Receipt> {
        let claim = receipt.inner.claim()?.value()?;
        let input = GuestInput::Singleton { self_image_id: self.set_builder_image_id, claim };
        let encoded_input = bytemuck::pod_collect_to_vec(&risc0_zkvm::serde::to_vec(&input)?);
        self.prove(
            self.set_builder_elf.clone(),
            encoded_input,
            vec![receipt],
            ProverOpts::succinct(),
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
        self.prove(
            self.assessor_elf.clone(),
            assessor_input.to_vec(),
            receipts,
            ProverOpts::succinct(),
        )
        .await
    }

    /// Fulfills an order as a singleton, returning the relevant data:
    /// * The [Fulfillment] of the order.
    /// * The [Receipt] of the root set.
    /// * The [SetInclusionReceipt] of the order.
    /// * The [SetInclusionReceipt] of the assessor.
    pub async fn fulfill(
        &self,
        order: Order,
        require_payment: bool,
    ) -> Result<(
        BoundlessFulfillment,
        Receipt,
        SetInclusionReceipt<ReceiptClaim>,
        SetInclusionReceipt<ReceiptClaim>,
    )> {
        let request = order.request.clone();
        let order_elf = fetch_url(&request.imageUrl).await?;
        let order_input: Vec<u8> = match request.input.inputType {
            InputType::Inline => InputEnv::unpack(&request.input.data)?.input(),
            InputType::Url => InputEnv::unpack(
                &fetch_url(
                    std::str::from_utf8(&request.input.data).context("input url is not utf8")?,
                )
                .await?,
            )?
            .input(),
            _ => bail!("Unsupported input type"),
        };
        let order_receipt =
            self.prove(order_elf.clone(), order_input, vec![], ProverOpts::succinct()).await?;
        let order_journal = order_receipt.journal.bytes.clone();
        let order_image_id = compute_image_id(&order_elf)?;
        let order_singleton = self.singleton(order_receipt.clone()).await?;

        let fill = Fulfillment {
            request: order.request.clone(),
            signature: order.signature.into(),
            journal: order_journal.clone(),
            require_payment,
        };

        let assessor_receipt = self.assessor(vec![fill], vec![order_receipt]).await?;
        let assessor_journal = assessor_receipt.journal.bytes.clone();
        let assessor_image_id = compute_image_id(&self.assessor_elf)?;
        let assessor_singleton = self.singleton(assessor_receipt).await?;

        let order_claim = ReceiptClaim::ok(order_image_id, order_journal.clone());
        let order_claim_digest = order_claim.digest();
        let assessor_claim = ReceiptClaim::ok(assessor_image_id, assessor_journal);
        let assessor_claim_digest = assessor_claim.digest();
        let root_receipt = self.join(order_singleton, assessor_singleton).await?;

        let order_path = merkle_path(&[order_claim_digest, assessor_claim_digest], 0);
        let assessor_path = merkle_path(&[order_claim_digest, assessor_claim_digest], 1);

        let verifier_parameters =
            SetInclusionReceiptVerifierParameters { image_id: self.set_builder_image_id };

        let order_inclusion_receipt = SetInclusionReceipt::from_path_with_verifier_params(
            order_claim,
            order_path,
            verifier_parameters.digest(),
        );
        let order_seal = order_inclusion_receipt.abi_encode_seal()?;

        let assessor_inclusion_receipt = SetInclusionReceipt::from_path_with_verifier_params(
            assessor_claim,
            assessor_path,
            verifier_parameters.digest(),
        );

        let fulfillment = BoundlessFulfillment {
            id: request.id,
            requestDigest: order.request.eip712_signing_hash(&self.domain.alloy_struct()),
            imageId: request.requirements.imageId,
            journal: order_journal.into(),
            requirePayment: require_payment,
            seal: order_seal.into(),
        };

        Ok((fulfillment, root_receipt, order_inclusion_receipt, assessor_inclusion_receipt))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::{primitives::PrimitiveSignature, signers::local::PrivateKeySigner};
    use boundless_market::contracts::{
        eip712_domain, Input, Offer, Predicate, ProofRequest, Requirements,
    };
    use guest_assessor::ASSESSOR_GUEST_ELF;
    use guest_set_builder::SET_BUILDER_ELF;
    use guest_util::{ECHO_ID, ECHO_PATH};
    use risc0_zkvm::VerifierContext;

    async fn setup_proving_request_and_signature(
        signer: &PrivateKeySigner,
    ) -> (ProofRequest, PrimitiveSignature) {
        let request = ProofRequest::new(
            0,
            &signer.address(),
            Requirements {
                imageId: <[u8; 32]>::from(Digest::from(ECHO_ID)).into(),
                predicate: Predicate::prefix_match(vec![1]),
            },
            &format!("file://{ECHO_PATH}"),
            Input::inline(vec![1, 2, 3, 4]),
            Offer::default(),
        );

        let signature = request.sign_request(signer, Address::ZERO, 1).await.unwrap();
        (request, signature)
    }

    #[ignore = "runs a proof; slow without RISC0_DEV_MODE=1"]
    #[tokio::test]
    async fn test_fulfill() {
        let signer = PrivateKeySigner::random();
        let (request, signature) = setup_proving_request_and_signature(&signer).await;

        let domain = eip712_domain(Address::ZERO, 1);
        let prover = DefaultProver::new(
            SET_BUILDER_ELF.to_vec(),
            ASSESSOR_GUEST_ELF.to_vec(),
            Address::ZERO,
            domain,
        )
        .expect("failed to create prover");

        let order = Order { request, signature };
        let (_, root_receipt, order_receipt, assessor_receipt) =
            prover.fulfill(order.clone(), false).await.unwrap();

        let verifier_parameters =
            SetInclusionReceiptVerifierParameters { image_id: prover.set_builder_image_id };

        order_receipt
            .with_root(root_receipt.clone())
            .verify_integrity_with_context(
                &VerifierContext::default(),
                verifier_parameters.clone(),
                None,
            )
            .unwrap();
        assessor_receipt
            .with_root(root_receipt.clone())
            .verify_integrity_with_context(&VerifierContext::default(), verifier_parameters, None)
            .unwrap();
    }
}
