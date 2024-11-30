// Copyright (c) 2024 RISC Zero, Inc.
//
// All rights reserved.

use alloy::{
    primitives::Address,
    sol_types::{SolStruct, SolValue},
};
use anyhow::{bail, Result};
use assessor::{AssessorInput, Fulfillment};
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
    order_stream_client::Order,
};

alloy::sol!(
    #[sol(all_derives)]
    struct OrderFulfilled {
        bytes32 root;
        bytes seal;
        BoundlessFulfillment[] fills;
        bytes assessorSeal;
        address prover;
    }
);

pub async fn fetch_url(url_str: &str) -> Result<Vec<u8>> {
    let url = Url::parse(url_str)?;

    match url.scheme() {
        "http" | "https" => fetch_http(&url).await,
        "file" => fetch_file(&url).await,
        _ => bail!("unsupported URL scheme: {}", url.scheme()),
    }
}

pub async fn fetch_http(url: &Url) -> Result<Vec<u8>> {
    let response = reqwest::get(url.as_str()).await?;
    let status = response.status();
    if !status.is_success() {
        bail!("HTTP request failed with status: {}", status);
    }

    Ok(response.bytes().await?.to_vec())
}

pub async fn fetch_file(url: &Url) -> Result<Vec<u8>> {
    let path = std::path::Path::new(url.path());
    let data = tokio::fs::read(path).await?;
    Ok(data)
}

pub struct DefaultProver {
    set_builder_elf: Vec<u8>,
    set_builder_image_id: Digest,
    assessor_elf: Vec<u8>,
    address: Address,
    domain: EIP721DomainSaltless,
}

impl DefaultProver {
    pub fn new(
        set_builder_elf: Vec<u8>,
        assessor_elf: Vec<u8>,
        address: Address,
        domain: EIP721DomainSaltless,
    ) -> Result<Self> {
        let set_builder_image_id = compute_image_id(&set_builder_elf)?;
        Ok(Self { set_builder_elf, set_builder_image_id, assessor_elf, address, domain })
    }

    pub async fn prove(
        &self,
        elf: Vec<u8>,
        input: Vec<u8>,
        assumptions: Vec<Receipt>,
    ) -> Result<Receipt> {
        let receipt = tokio::task::spawn_blocking(move || {
            let mut env = ExecutorEnv::builder();
            env.write_slice(&input);
            for assumption_receipt in assumptions.iter() {
                env.add_assumption(assumption_receipt.clone());
            }
            let env = env.build()?;
            default_prover().prove_with_opts(env, &elf, &ProverOpts::succinct())
        })
        .await??
        .receipt;
        Ok(receipt)
    }

    pub async fn prove_compress(
        &self,
        elf: Vec<u8>,
        input: Vec<u8>,
        assumptions: Vec<Receipt>,
    ) -> Result<Receipt> {
        let receipt = tokio::task::spawn_blocking(move || {
            let mut env = ExecutorEnv::builder();
            env.write_slice(&input);
            for assumption_receipt in assumptions.iter() {
                env.add_assumption(assumption_receipt.clone());
            }
            let env = env.build()?;
            default_prover().prove_with_opts(env, &elf, &ProverOpts::groth16())
        })
        .await??
        .receipt;
        Ok(receipt)
    }

    pub async fn join(&self, left: Receipt, right: Receipt) -> Result<Receipt> {
        let left_output = <GuestOutput>::abi_decode(&left.journal.bytes, true)?;
        let right_output = <GuestOutput>::abi_decode(&right.journal.bytes, true)?;
        let input = GuestInput::Join {
            self_image_id: self.set_builder_image_id,
            left_set_root: left_output.root(),
            right_set_root: right_output.root(),
        };
        let encoded_input = bytemuck::pod_collect_to_vec(&risc0_zkvm::serde::to_vec(&input)?);
        self.prove_compress(self.set_builder_elf.clone(), encoded_input, vec![left, right]).await
    }

    pub async fn singleton(&self, receipt: Receipt) -> Result<Receipt> {
        let claim = receipt.inner.claim()?.value()?;
        let input = GuestInput::Singleton { self_image_id: self.set_builder_image_id, claim };
        let encoded_input = bytemuck::pod_collect_to_vec(&risc0_zkvm::serde::to_vec(&input)?);
        self.prove(self.set_builder_elf.clone(), encoded_input, vec![receipt]).await
    }

    pub async fn assessor(
        &self,
        fills: Vec<Fulfillment>,
        receipts: Vec<Receipt>,
    ) -> Result<Receipt> {
        let assessor_input =
            AssessorInput { domain: self.domain.clone(), fills, prover_address: self.address };
        self.prove(self.assessor_elf.clone(), assessor_input.to_vec(), receipts).await
    }

    pub async fn fulfill(&self, order: Order, require_payment: bool) -> Result<OrderFulfilled> {
        let request = order.request.clone();
        let order_elf = fetch_url(&request.imageUrl).await?;
        let order_input: Vec<u8> = match request.input.inputType {
            InputType::Inline => request.input.data.into(),
            InputType::Url => fetch_url(&request.input.data.to_string()).await?.into(),
            _ => bail!("Unsupported input type"),
        };
        let order_receipt = self.prove(order_elf.clone(), order_input, vec![]).await?;
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
        let root = <GuestOutput>::abi_decode(&root_receipt.journal.bytes, true)?.root();
        let root_seal = encode_seal(&root_receipt)?;

        let order_path = merkle_path(&[order_claim_digest, assessor_claim_digest], 0);
        let assessor_path = merkle_path(&[order_claim_digest, assessor_claim_digest], 1);

        let verifier_parameters =
            SetInclusionReceiptVerifierParameters { image_id: self.set_builder_image_id };

        let mut order_inclusion_receipt =
            SetInclusionReceipt::from_path(order_claim, order_path);
        order_inclusion_receipt.verifier_parameters = verifier_parameters.digest();
        let order_seal = order_inclusion_receipt.abi_encode_seal()?;

        let mut assessor_inclusion_receipt =
            SetInclusionReceipt::from_path(assessor_claim, assessor_path);
        assessor_inclusion_receipt.verifier_parameters = verifier_parameters.digest();
        let assessor_seal = assessor_inclusion_receipt.abi_encode_seal()?;

        let fulfillment = BoundlessFulfillment {
            id: request.id,
            requestDigest: order.request.eip712_signing_hash(&self.domain.alloy_struct()),
            imageId: request.requirements.imageId,
            journal: order_journal.into(),
            requirePayment: require_payment,
            seal: order_seal.into(),
        };

        Ok(OrderFulfilled {
            root: <[u8; 32]>::from(root).into(),
            seal: root_seal.into(),
            fills: vec![fulfillment],
            assessorSeal: assessor_seal.into(),
            prover: self.address,
        })
    }
}
