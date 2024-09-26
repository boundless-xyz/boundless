// Copyright (c) 2024 RISC Zero, Inc.
//
// All rights reserved.

use alloy_primitives::B256;
use sha2::{Digest as _, Sha256};

pub fn verify_inclusion_sha256(proof: Vec<u8>, root: B256, leaf: B256, index: u64) -> bool {
    let len = proof.len();
    process_inclusion_proof_sha256(proof, leaf, index, len) == root
}

pub fn process_inclusion_proof_sha256(proof: Vec<u8>, leaf: B256, index: u64, len: usize) -> B256 {
    assert!(
        !proof.is_empty() && proof.len() % 32 == 0,
        "proof length should be a non-zero multiple of 32"
    );
    let mut index = index;
    assert!(index < 2u64.pow(proof.len() as u32 / 32), "index out of bounds");

    let mut computed_hash = leaf.as_slice().to_vec();
    for i in 0..len / 32 {
        let hash_input: Vec<u8> = if index % 2 == 0 {
            // if ith bit of index is 0, then computed_hash is a left sibling
            [computed_hash.as_slice(), &proof[i * 32..(i + 1) * 32]].concat()
        } else {
            // if ith bit of index is 1, then computed_hash is a right sibling
            [&proof[i * 32..(i + 1) * 32], computed_hash.as_slice()].concat()
        };

        let mut hasher = Sha256::new();
        hasher.update(hash_input);
        computed_hash = hasher.finalize().to_vec(); // sha256_hash needs to be defined
        index /= 2;
    }

    B256::try_from(computed_hash.as_slice()).unwrap()
}

// returns the root hash of a merkle tree with the given leaves
// requires all hashes to be 32 bytes
// precondition: leaves.len() to be a power of 2
pub fn merkleize_sha256(leaves: &[B256]) -> B256 {
    assert!(leaves.len() % 2 == 0, "Number of leaves must be even");

    let mut num_nodes_in_layer = leaves.len() / 2;
    let mut layer: Vec<B256> = Vec::with_capacity(num_nodes_in_layer);

    for i in 0..num_nodes_in_layer {
        let mut hasher = Sha256::new();
        hasher.update(leaves[2 * i].as_slice());
        hasher.update(leaves[2 * i + 1].as_slice());
        layer.push(B256::try_from(hasher.finalize().as_slice()).unwrap());
    }

    while num_nodes_in_layer != 1 {
        num_nodes_in_layer /= 2;
        for i in 0..num_nodes_in_layer {
            let mut hasher = Sha256::new();
            hasher.update(layer[2 * i].as_slice());
            hasher.update(layer[2 * i + 1].as_slice());
            layer[i] = B256::try_from(hasher.finalize().as_slice()).unwrap();
        }
        layer.truncate(num_nodes_in_layer);
    }

    layer[0]
}
