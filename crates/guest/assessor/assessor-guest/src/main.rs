// Copyright (c) 2025 RISC Zero, Inc.
//
// All rights reserved.

// TODO(victor) I don't think steel is no_std
#![no_main]
#![no_std]

extern crate alloc;

use alloc::{vec, vec::Vec};
use alloy_primitives::{Address, FixedBytes, B256, U256};
use alloy_sol_types::SolValue;
use boundless_assessor::{steel::SteelVerifier, AssessorInput, Authorization};
use boundless_market::contracts::{AssessorCallback, AssessorJournal, Selector, SteelCommitment};
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

    let eip_domain_separator = input.domain.alloy_struct();
    let auth_root_env = input.authorization_root.map(|evm_input| evm_input.into_env());

    // list of request digests
    let mut request_digests: Vec<B256> = Vec::with_capacity(input.fills.len());
    // list of ReceiptClaim digests used as leaves in the aggregation set
    let mut claim_digests: Vec<Digest> = Vec::with_capacity(input.fills.len());
    // sparse list of callbacks to be recorded in the journal
    let mut callbacks: Vec<AssessorCallback> = Vec::<AssessorCallback>::new();
    // list of optional Selectors specified as part of the requests requirements
    let mut selectors = Vec::<Selector>::new();

    // For each fill we
    // - verify the request's signature
    // - evaluate the request's requirements
    // - verify the integrity of its claim
    // - record the callback if it exists
    // - record the selector if it is present
    // We additionally collect the request and claim digests.
    // NOTE: Destructure input to allow for consumption of authorization list.
    let AssessorInput { fills, authorization, prover_address, .. } = input;
    assert_eq!(fills.len(), authorization.len()); // ensure zip inputs are equal length
    for ((index, fill), auth) in core::iter::zip(fills.iter().enumerate(), authorization) {
        // Verify authorization of the request, handling the case where only ECDSA signatures are
        // provided and the SteelVerifier is unecessary.
        let request_digest = match auth_root_env {
            Some(ref evm_env) => fill
                .verify_auth(&eip_domain_separator, auth, &SteelVerifier::new(evm_env))
                .expect("authorization does not verify"),
            None => {
                let Authorization::ECDSA(ref signature) = auth else {
                    panic!("no authorization_root provided to verify request authorization; can only verify ECDSA signatures");
                };
                fill.verify_ecdsa(&eip_domain_separator, signature)
                    .expect("ECDSA signature does not verify")
            }
        };

        env::verify_integrity(&fill.receipt_claim()).expect("claim integrity check failed");
        fill.evaluate_requirements().expect("requirements not met");

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

    // Compute the merkle root of the claim digests
    let root = merkle_root(&claim_digests);

    // Produce the Steel commitment that wull be verified onchain.
    // A commitment of all zeroes is used to represent None. Note that a commitment of all zeroes
    // cannot correspond to any block, as no block hashes to the zero digest.
    // HACK: The SteelCommitment used in AssessorJournal and steel::Commitment are both Solidity
    // structs with the same fields, but are not the same types at the Rust level because they are
    // generated in seperate sources with the sol macro. Here we do a simply type cast to work
    // around this.
    let steel_commitment = auth_root_env
        .map(|env| {
            let commit = env.into_commitment();
            SteelCommitment { id: commit.id, digest: commit.digest, configID: commit.configID }
        })
        .unwrap_or(SteelCommitment { id: U256::ZERO, digest: B256::ZERO, configID: B256::ZERO });

    let journal = AssessorJournal {
        requestDigests: request_digests,
        callbacks,
        selectors,
        root: <[u8; 32]>::from(root).into(),
        prover: prover_address,
        steel_commitment,
    };

    env::commit_slice(&journal.abi_encode());
}
