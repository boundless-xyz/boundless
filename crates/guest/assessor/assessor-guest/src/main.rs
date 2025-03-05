// Copyright (c) 2024 RISC Zero, Inc.
//
// All rights reserved.

#![no_main]
#![no_std]

extern crate alloc;

use alloc::{vec, vec::Vec};
use alloy_primitives::B256;
use alloy_sol_types::sol;
use alloy_sol_types::SolValue;
use boundless_assessor::AssessorInput;
use boundless_market::contracts::AssessorJournal;
use risc0_aggregation::merkle_root;
use risc0_zkvm::{
    guest::env,
    sha::{Digest, Digestible},
};

risc0_zkvm::guest::entry!(main);

sol! {
    interface IERC1271 {
        function isValidSignature(
            bytes32 _hash,
            bytes memory _signature)
            public
            view
            returns (bytes4 magicValue);
    }
}

const ERC1271_MAGICVALUE: [u8; 4] = [0x16, 0x26, 0xba, 0x7e];

fn main() {
    let mut len: u32 = 0;
    env::read_slice(core::slice::from_mut(&mut len));
    let mut bytes = vec![0u8; len as usize];
    env::read_slice(&mut bytes);
    
    let eth_evm_input: EthEvmInput = env::read();
    let env = eth_evm_input.into_env().with_chain_spec(&ETH_SEPOLIA_CHAIN_SPEC);

    let input: AssessorInput = postcard::from_bytes(&bytes).expect("failed to deserialize input");

    // list of request digests
    let mut request_digests: Vec<B256> = Vec::with_capacity(input.fills.len());
    // list of ReceiptClaim digests used as leaves in the aggregation set
    let mut claim_digests: Vec<Digest> = Vec::with_capacity(input.fills.len());

    let eip_domain_separator = input.domain.alloy_struct();
    // For each fill we
    // - verify the request's signature
    // - evaluate the request's requirements
    // - verify the integrity of its claim
    // We additionally collect the request and claim digests.
    for fill in input.fills.iter() {
        let request_digest =
            fill.verify_signature(&eip_domain_separator, &env).expect("signature does not verify");
        fill.evaluate_requirements().expect("requirements not met");
        env::verify_integrity(&fill.receipt_claim()).expect("claim integrity check failed");
        claim_digests.push(fill.receipt_claim().digest());
        request_digests.push(request_digest.into());
    }

    // recompute the merkle root of the aggregation set
    let root = merkle_root(&claim_digests);

    let journal = AssessorJournal {
        commitment: env.into_commitment(),
        requestDigests: request_digests,
        root: <[u8; 32]>::from(root).into(),
        prover: input.prover_address,
    };

    env::commit_slice(&journal.abi_encode());
}
