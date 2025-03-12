// Copyright (c) 2025 RISC Zero, Inc.
//
// All rights reserved.

#![no_main]
#![no_std]

extern crate alloc;

use alloc::{vec, vec::Vec};
use alloy_primitives::{Address, FixedBytes, B256};
use alloy_sol_types::SolStruct;
use alloy_sol_types::SolValue;
use boundless_assessor::AssessorInput;
use boundless_market::contracts::{AssessorCallback, AssessorJournal, RequestId, Selector};
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

    // Ensure that the number of fills is within the bounds of the supported max set size.
    // This limitation is imposed by the Selector struct, which uses a u16 to store the index of the
    // fill in the set.
    if input.fills.len() >= u16::MAX.into() {
        panic!("too many fills: {}", input.fills.len());
    }

    // list of request digests
    let mut request_digests: Vec<B256> = Vec::with_capacity(input.fills.len());
    // list of ReceiptClaim digests used as leaves in the aggregation set
    let mut claim_digests: Vec<Digest> = Vec::with_capacity(input.fills.len());
    // sparse list of callbacks to be recorded in the journal
    let mut callbacks: Vec<AssessorCallback> = Vec::<AssessorCallback>::new();
    // list of optional Selectors specified as part of the requests requirements
    let mut selectors = Vec::<Selector>::new();

    let eip_domain_separator = input.domain.alloy_struct();
    // For each fill we
    // - verify the request's signature
    // - evaluate the request's requirements
    // - verify the integrity of its claim
    // - record the callback if it exists
    // - record the selector if it is present
    // We additionally collect the request and claim digests.
    for (index, fill) in input.fills.iter().enumerate() {
        // ECDSA signatures are always checked here.
        // Smart contract signatures (via EIP-1271) are checked on-chain either when a request is locked,
        // or when an unlocked request is priced and fulfilled.
        let smart_contract_signed = RequestId::try_from(fill.request.id).unwrap().smart_contract_signed;
        let request_digest: [u8; 32];
        if smart_contract_signed {
            request_digest = fill.request.eip712_signing_hash(&eip_domain_separator).into();
        } else {
            request_digest =
                fill.verify_signature(&eip_domain_separator).expect("signature does not verify");
        }
        fill.evaluate_requirements().expect("requirements not met");
        env::verify_integrity(&fill.receipt_claim()).expect("claim integrity check failed");
        claim_digests.push(fill.receipt_claim().digest());
        request_digests.push(request_digest.into());
        if fill.request.requirements.callback.addr != Address::ZERO {
            callbacks.push(AssessorCallback {
                index: index.try_into().expect("callback index overflow"),
                addr: fill.request.requirements.callback.addr,
                gasLimit: fill.request.requirements.callback.gasLimit,
            });
        }
        if fill.request.requirements.selector != FixedBytes::<4>([0; 4]) {
            selectors.push(Selector {
                index: index.try_into().expect("selector index overflow"),
                value: fill.request.requirements.selector,
            });
        }
    }

    // recompute the merkle root of the aggregation set
    let root = merkle_root(&claim_digests);

    let journal = AssessorJournal {
        requestDigests: request_digests,
        callbacks,
        selectors,
        root: <[u8; 32]>::from(root).into(),
        prover: input.prover_address,
    };

    env::commit_slice(&journal.abi_encode());
}
