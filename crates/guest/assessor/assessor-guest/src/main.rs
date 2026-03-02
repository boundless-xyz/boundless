// Copyright 2026 Boundless Foundation, Inc.
//
// Use of this source code is governed by the Business Source License
// as found in the LICENSE-BSL file.

#![no_main]
#![no_std]

extern crate alloc;

use alloc::vec::Vec;
use alloy_primitives::{Address, U256};
use alloy_sol_types::{SolStruct, SolValue};
use borsh::BorshDeserialize;
use boundless_assessor::{process_tree, AssessorInput, AssessorPovwClaim};
use boundless_market::contracts::{
    AssessorCallback, AssessorCommitment, AssessorJournal, AssessorSelector, FulfillmentData,
    PoVWClaim, RequestId, UNSPECIFIED_SELECTOR,
};
use risc0_zkvm::{guest::env, sha::{Digest, Digestible}, ReceiptClaim, WorkClaim};

risc0_zkvm::guest::entry!(main);

fn main() {
    let bytes = env::read_frame();
    let input = AssessorInput::decode(&bytes).expect("failed to deserialize input");

    // Ensure that the number of fills is within the bounds of the supported max set size.
    // This limitation is imposed by the Selector struct, which uses a u16 to store the index of the
    // fill in the set.
    if input.fills.len() >= u16::MAX.into() {
        panic!("too many fills: {}", input.fills.len());
    }

    // list of ReceiptClaim digests used as leaves in the aggregation set
    let mut leaves: Vec<Digest> = Vec::with_capacity(input.fills.len());
    // claim digest per fill, used for PoVW binding (parallel to fills)
    let mut claim_digests: Vec<Digest> = Vec::with_capacity(input.fills.len());
    // sparse list of callbacks to be recorded in the journal
    let mut callbacks: Vec<AssessorCallback> = Vec::<AssessorCallback>::new();
    // list of optional Selectors specified as part of the requests requirements
    let mut selectors = Vec::<AssessorSelector>::new();

    let eip_domain_separator = input.domain.alloy_struct();
    // For each fill we
    // - verify the request's signature
    // - evaluate the request's requirements
    // - record the callback if it exists
    // - record the selector if it is present
    // NOTE: We do not verify soundness of the claim. That is done on chain.
    // We additionally collect the request and claim digests.
    for (index, fill) in input.fills.iter().enumerate() {
        // Attempt to decode the request ID. If this fails, there may be flags that are not handled
        // by this guest. This check is not strictly needed, but reduces the chance of accidentally
        // failing to enforce a constraint.
        RequestId::try_from(fill.request.id).unwrap();
        fill.request.validate().expect("request is not valid");

        // ECDSA signatures are always checked here.
        // Smart contract signatures (via EIP-1271) are checked on-chain either when a request is locked,
        // or when an unlocked request is priced and fulfilled
        let request_digest: [u8; 32] = if fill.request.is_smart_contract_signed() {
            fill.request.eip712_signing_hash(&eip_domain_separator).into()
        } else {
            fill.verify_signature(&eip_domain_separator).expect("signature does not verify")
        };
        let claim_digest = fill.evaluate_requirements().expect("requirements not met");
        claim_digests.push(claim_digest);
        let commit = AssessorCommitment {
            index: U256::from(index),
            id: fill.request.id,
            requestDigest: request_digest.into(),
            claimDigest: <[u8; 32]>::from(claim_digest).into(),
            fulfillmentDataDigest: <[u8; 32]>::from(
                fill.fulfillment_data
                    .fulfillment_data_digest()
                    .expect("failed to get fulfillment data digest"),
            )
            .into(),
        }
        .eip712_hash_struct();
        leaves.push(Digest::from_bytes(*commit));

        let callback = &fill.request.requirements.callback;

        if callback.addr != Address::ZERO {
            match &fill.fulfillment_data {
                FulfillmentData::ImageIdAndJournal(_, _) => {}
                _ => panic!("callback requested but no fulfillment data provided"),
            }
            callbacks.push(AssessorCallback {
                index: index.try_into().expect("callback index overflow"),
                addr: callback.addr,
                gasLimit: callback.gasLimit,
            });
        }

        if fill.request.requirements.selector != UNSPECIFIED_SELECTOR {
            selectors.push(AssessorSelector {
                index: index.try_into().expect("selector index overflow"),
                value: fill.request.requirements.selector,
            });
        }
    }

    // compute the merkle root of the commitments
    let root = process_tree(leaves);

    // PoVW binding: for each provided work claim, verify the assumption inline and bind it to the
    // corresponding fill via the committed claim digest.
    // The output is a dense array parallel to fills (cycleCount = 0 for unattributed fills).
    // If absent, workLogId = address(0) and povwClaims = [] — backward compatible.
    let (work_log_id, povw_claims) = if let Some(ref povw) = input.povw {
        // Build a dense cycle-count array indexed by fill position.
        let mut cycle_counts = alloc::vec![0u64; input.fills.len()];
        for AssessorPovwClaim { index, work_claim } in &povw.claims {
            let fill_idx = *index as usize;
            let work_claim: WorkClaim<ReceiptClaim> =
                WorkClaim::deserialize_reader(&mut work_claim.as_ref())
                    .expect("failed to decode WorkClaim");
            // Verify the work claim receipt (host must add it as a zkVM assumption).
            env::verify_assumption(work_claim.digest(), Digest::ZERO)
                .expect("work claim verification failed");
            // Bind: the WorkClaim's ReceiptClaim digest must match the committed claim digest for
            // this fill, cryptographically tying the cycle count to this specific execution.
            let rc = work_claim.claim.as_value().expect("work claim.claim pruned");
            assert_eq!(
                rc.digest(),
                claim_digests[fill_idx],
                "work claim claimDigest does not match fill at index {fill_idx}",
            );
            cycle_counts[fill_idx] =
                work_claim.work.as_value().expect("work claim.work pruned").value;
        }
        let claims = cycle_counts.into_iter().map(|c| PoVWClaim { cycleCount: c }).collect();
        (povw.log_id, claims)
    } else {
        (Address::ZERO, Vec::new())
    };

    let journal = AssessorJournal {
        callbacks,
        selectors,
        root: <[u8; 32]>::from(root).into(),
        prover: input.prover_address,
        workLogId: work_log_id,
        povwClaims: povw_claims,
    };

    env::commit_slice(&journal.abi_encode());
}
