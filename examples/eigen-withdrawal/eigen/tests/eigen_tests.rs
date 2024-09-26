// Copyright (c) 2024 RISC Zero, Inc.
//
// All rights reserved.

use std::{fs::File, io::Read};

use alloy_sol_types::SolType;
use eigen::withdrawal::{
    WithdrawalJournal, WithdrawalJournals, WithdrawalProof, WithdrawalProofJson,
};
use methods::{
    EIGEN_WITHDRAWAL_AGGR_ELF, EIGEN_WITHDRAWAL_AGGR_ID, EIGEN_WITHDRAWAL_ELF, EIGEN_WITHDRAWAL_ID,
};
use risc0_zkvm::{default_prover, sha::Digestible, ExecutorEnv};
use test_log::test;

#[test]
fn validate_proof() {
    let mut file =
        File::open("tests/test-data/partialWithdrawalProof_Latest.json").expect("File not found");
    let mut contents = String::new();
    file.read_to_string(&mut contents).expect("Something went wrong reading the file");

    // Parse the string contents into the data structure
    let proof_json: WithdrawalProofJson = serde_json::from_str(&contents).unwrap();
    let proof: WithdrawalProof = proof_json.into();
    proof.verify_withdrawal().unwrap();
    proof.verify_validator_fields().unwrap();
}

#[test]
fn validate_proof_aggr_guest() {
    let mut file =
        File::open("tests/test-data/partialWithdrawalProof_Latest.json").expect("File not found");
    let mut contents = String::new();
    file.read_to_string(&mut contents).expect("Something went wrong reading the file");

    // Parse the string contents into the data structure
    let proof_json: WithdrawalProofJson = serde_json::from_str(&contents).unwrap();
    let proof: WithdrawalProof = proof_json.into();
    let proofs = vec![proof];

    let env = ExecutorEnv::builder().write(&proofs).unwrap().build().unwrap();

    let receipt = default_prover().prove(env, EIGEN_WITHDRAWAL_AGGR_ELF).unwrap().receipt;
    receipt.verify(EIGEN_WITHDRAWAL_AGGR_ID).unwrap();
    let journal_digest = receipt.journal.digest();
    let decoded_journal = <WithdrawalJournals>::abi_decode(&receipt.journal.bytes, true).unwrap();
    assert_eq!(journal_digest, decoded_journal.digest());
}

#[test]
fn validate_proof_guest() {
    let mut file =
        File::open("tests/test-data/partialWithdrawalProof_Latest.json").expect("File not found");
    let mut contents = String::new();
    file.read_to_string(&mut contents).expect("Something went wrong reading the file");

    // Parse the string contents into the data structure
    let proof_json: WithdrawalProofJson = serde_json::from_str(&contents).unwrap();
    let proof: WithdrawalProof = proof_json.into();

    let env = ExecutorEnv::builder().write(&proof).unwrap().build().unwrap();

    let receipt = default_prover().prove(env, EIGEN_WITHDRAWAL_ELF).unwrap().receipt;
    receipt.verify(EIGEN_WITHDRAWAL_ID).unwrap();
    let journal_digest = receipt.journal.digest();
    let decoded_journal = <WithdrawalJournal>::abi_decode(&receipt.journal.bytes, true).unwrap();
    assert_eq!(journal_digest, decoded_journal.digest());
}
