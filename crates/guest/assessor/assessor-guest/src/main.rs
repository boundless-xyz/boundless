// Copyright (c) 2024 RISC Zero, Inc.
//
// All rights reserved.

#![no_main]
#![no_std]

extern crate alloc;

use alloc::{vec, vec::Vec};
use alloy_primitives::{Address, B256};
use alloy_sol_types::SolValue;
use boundless_assessor::AssessorInput;
use boundless_market::contracts::{AssessorJournal, AssessorJournalCallback};
use risc0_aggregation::merkle_root;
use risc0_zkvm::{
    guest::env,
    sha::{Digest, Digestible},
};

risc0_zkvm::guest::entry!(main);

fn main() {
    let mut len: u32 = 0;
    env::read_slice(core::slice::from_mut(&mut len));
    let mut bytes = vec![0u8; len as usize];
    env::read_slice(&mut bytes);

    let input: AssessorInput = postcard::from_bytes(&bytes).expect("failed to deserialize input");

    // list of request digests
    let mut request_digests: Vec<B256> = Vec::with_capacity(input.fills.len());
    // list of ReceiptClaim digests used as leaves in the aggregation set
    let mut claim_digests: Vec<Digest> = Vec::with_capacity(input.fills.len());
    // sparse list of callbacks to be recorded in the journal
    let mut callbacks: Vec<AssessorJournalCallback> = Vec::<AssessorJournalCallback>::new();

    let eip_domain_separator = input.domain.alloy_struct();
    // For each fill we
    // - verify the request's signature
    // - evaluate the request's requirements
    // - verify the integrity of its claim
    // - record the callback if it exists
    // We additionally collect the request and claim digests.
    for (index, fill) in input.fills.iter().enumerate() {
        let request_digest =
            fill.verify_signature(&eip_domain_separator).expect("signature does not verify");
        fill.evaluate_requirements().expect("requirements not met");
        env::verify_integrity(&fill.receipt_claim()).expect("claim integrity check failed");
        claim_digests.push(fill.receipt_claim().digest());
        request_digests.push(request_digest.into());
        if fill.request.requirements.callback.addr != Address::ZERO {
            callbacks.push(AssessorJournalCallback {
                index: index.try_into().expect("callback index overflow"),
                addr: fill.request.requirements.callback.addr,
                gasLimit: fill.request.requirements.callback.gasLimit,
            });
        }
    }

    // recompute the merkle root of the aggregation set
    let root = merkle_root(&claim_digests);

    let journal = AssessorJournal {
        requestDigests: request_digests,
        callbacks,
        root: <[u8; 32]>::from(root).into(),
        prover: input.prover_address,
    };

    env::commit_slice(&journal.abi_encode());
}
