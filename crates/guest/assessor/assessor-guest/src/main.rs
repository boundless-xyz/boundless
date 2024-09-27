// Copyright (c) 2024 RISC Zero, Inc.
//
// All rights reserved.

#![no_main]
#![no_std]

extern crate alloc;

use aggregation_set::AGGREGATION_SET_GUEST_ID;
use aggregation_set::{merkle_root, GuestOutput};
use alloc::{vec, vec::Vec};
use alloy_primitives::aliases::U192;
use alloy_sol_types::SolValue;
use assessor::AssessorInput;
use boundless_market::contracts::AssessorJournal;
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

    // list of request ids
    let mut ids: Vec<U192> = Vec::with_capacity(input.fills.len());
    // list of ReceiptClaim digests used as leaves in the aggregation set
    let mut leaves: Vec<Digest> = Vec::with_capacity(input.fills.len());

    let eip_domain_separator = input.domain.alloy_struct();
    // For each claim we
    // - verify the proving request's signature
    // - evaluate the request's requirements
    // We additionally collect the request ids and the digests from the claims
    for fill in input.fills.iter() {
        fill.verify_signature(&eip_domain_separator).expect("signature does not verify");
        fill.evaluate_requirements().expect("requirements not met");
        ids.push(fill.request.id);
        leaves.push(fill.receipt_claim().digest());
    }

    // recompute the merkle root of the aggregation set
    let root = merkle_root(&leaves).expect("failed to compute merkle root");
    let aggregation_set_id = Digest::from(AGGREGATION_SET_GUEST_ID);
    let journal = GuestOutput::new(aggregation_set_id, root).abi_encode();

    // verify the integrity of the committed root
    env::verify(aggregation_set_id, &journal).expect("aggregation set root does not verify");

    let journal = AssessorJournal {
        requestIds: ids,
        root: <[u8; 32]>::from(root).into(),
        eip712DomainSeparator: eip_domain_separator.hash_struct(),
    };

    env::commit_slice(&journal.abi_encode());
}
