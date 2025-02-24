// Copyright (c) 2025 RISC Zero, Inc.
//
// All rights reserved.

#![no_main]
#![no_std]

extern crate alloc;

use alloy_sol_types::SolValue;
use boundless_market::contracts::ResolveJournal;
use boundless_resolve::{AssumptionReceipt, ResolveInput};
use risc0_aggregation::{RecursionVerifierParamters, SetInclusionReceiptVerifierParameters};
use risc0_zkvm::{
    guest::env,
    sha::{Digest, Digestible},
    Assumption, Groth16ReceiptVerifierParameters, SuccinctReceiptVerifierParameters,
    VerifierContext,
};

risc0_zkvm::guest::entry!(main);

fn main() {
    // Read and deserialize the input.
    let bytes = env::read_frame();
    let input: ResolveInput = postcard::from_bytes(&bytes).expect("failed to deserialize input");

    // Verify the integrity of the conditional claim.
    // NOTE: We do not use `verify_integrity' here because that method does not allow verification
    // of conditional receipts. This is to prevent a footgun, where verifying a conditional receipt
    // tells you nothing unless you also verify the assumptions. Here, we also verify the assumption
    // by composition and commit the resolved commit claim, making it semantically consistent.
    env::verify_assumption(input.conditional.digest(), Digest::ZERO).unwrap();

    // Process the conditional claim and extract the head assumption.
    let mut claim = input.conditional;
    let output = claim.output.as_value_mut().expect("Output pruned").as_mut().expect("No output");
    let assumptions = output.assumptions.as_value_mut().expect("Assumptions pruned");
    if assumptions.0.is_empty() {
        panic!("No assumptions")
    }
    let assumption = assumptions.0.remove(0);
    let assumption = assumption.as_value().expect("Assumption pruned");

    // Create verifier context and verify assumption.
    let verifier_context = create_verifier_context(assumption.control_root);
    verify_assumption(input.set_builder_image_id, input.assumption, assumption, &verifier_context);

    // Commit the resolve journal.
    let journal = ResolveJournal {
        setBuilderImageID: <[u8; 32]>::from(input.set_builder_image_id).into(),
        claimDigest: <[u8; 32]>::from(claim.digest()).into(),
    };

    env::commit_slice(&journal.abi_encode());
}

/// Construct a verifier context that accepts receipts corresponding to the specified control root, or the default control root if the specified root is zero.
// TODO(victor) Add a `with_control_root` builder method on VerifierContext to do this.
fn create_verifier_context(control_root: Digest) -> VerifierContext {
    let succinct_params = {
        let mut params = SuccinctReceiptVerifierParameters::default();
        if control_root != Digest::ZERO {
            params.control_root = control_root;
        }
        params
    };

    let groth16_params = {
        let mut params = Groth16ReceiptVerifierParameters::default();
        if control_root != Digest::ZERO {
            params.control_root = control_root;
        }
        params
    };

    VerifierContext::empty()
        .with_suites(VerifierContext::default_hash_suites())
        .with_succinct_verifier_parameters(succinct_params)
        .with_groth16_verifier_parameters(groth16_params)
}

/// Verifies the assumption receipt against the expected assumption.
fn verify_assumption(
    set_builder_image_id: Digest,
    assumption_receipt: AssumptionReceipt,
    assumption: &Assumption,
    verifier_context: &VerifierContext,
) {
    match assumption_receipt {
        AssumptionReceipt::Base(risc0_zkvm::AssumptionReceipt::Unresolved(ref unresolved)) => {
            assert_eq!(
                unresolved, assumption,
                "provided assumption is not equal to the head of the list: {} != {}",
                unresolved.claim, assumption.claim,
            );
            env::verify_assumption(assumption.claim, assumption.control_root).unwrap();
        }
        AssumptionReceipt::Base(risc0_zkvm::AssumptionReceipt::Proven(receipt)) => {
            let receipt_claim_digest = receipt.claim_digest().unwrap();
            assert_eq!(
                receipt_claim_digest, assumption.claim,
                "provided assumption is not equal to the head of the list: {} != {}",
                receipt_claim_digest, assumption.claim,
            );
            receipt.verify_integrity_with_context(verifier_context).unwrap();
        }
        AssumptionReceipt::SetInclusion(receipt) => {
            let receipt_claim_digest = receipt.claim.digest();
            assert_eq!(
                receipt_claim_digest, assumption.claim,
                "provided assumption is not equal to the head of the list: {} != {}",
                receipt_claim_digest, assumption.claim,
            );
            receipt
                .verify_integrity_with_context(
                    verifier_context,
                    SetInclusionReceiptVerifierParameters { image_id: set_builder_image_id },
                    Some(RecursionVerifierParamters {
                        control_root: (assumption.control_root != Digest::ZERO)
                            .then_some(assumption.control_root),
                    }),
                )
                .unwrap();
        }
        _ => unimplemented!("assumption receipt type is not supported"),
    }
}
