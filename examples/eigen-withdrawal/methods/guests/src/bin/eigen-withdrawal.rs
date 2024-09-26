// Copyright (c) 2024 RISC Zero, Inc.
//
// All rights reserved.

#![no_main]

use alloy_sol_types::SolValue;
use eigen::withdrawal::{WithdrawalJournal, WithdrawalProof};
use risc0_zkvm::guest::env;

risc0_zkvm::guest::entry!(main);

pub fn main() {
    let proof: WithdrawalProof = env::read();
    proof.verify_withdrawal().unwrap();
    proof.verify_validator_fields().unwrap();
    let journal = WithdrawalJournal::from(proof);

    env::commit_slice(&journal.abi_encode());
}
