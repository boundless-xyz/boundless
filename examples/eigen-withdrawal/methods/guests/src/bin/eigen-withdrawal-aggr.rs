// Copyright (c) 2024 RISC Zero, Inc.
//
// All rights reserved.

#![no_main]

use alloy_sol_types::SolValue;
use eigen::withdrawal::{WithdrawalJournal, WithdrawalJournals, WithdrawalProof};
use risc0_zkvm::guest::env;

risc0_zkvm::guest::entry!(main);

pub fn main() {
    let proofs: Vec<WithdrawalProof> = env::read();
    let mut withdrawal_journals = WithdrawalJournals { journals: Vec::new() };
    for proof in proofs {
        proof.verify_withdrawal().unwrap();
        proof.verify_validator_fields().unwrap();
        withdrawal_journals.journals.push(WithdrawalJournal::from(proof));
    }
    env::commit_slice(&withdrawal_journals.abi_encode());
}
