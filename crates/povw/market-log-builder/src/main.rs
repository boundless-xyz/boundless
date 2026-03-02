// Copyright 2026 Boundless Foundation, Inc.
//
// Use of this source code is governed by the Business Source License
// as found in the LICENSE-BSL file.

#![no_main]
#![no_std]

extern crate alloc;

use alloy_sol_types::SolValue;
use borsh::BorshDeserialize;
use boundless_povw::market_log_builder::{MarketLogBuilderInput, MarketLogBuilderJournal};
use risc0_zkvm::guest::env;
use risc0_zkvm::sha::Digestible;
use risc0_zkvm::{Digest, ReceiptClaim, WorkClaim};

risc0_zkvm::guest::entry!(main);

fn main() {
    let input = MarketLogBuilderInput::decode(env::read_frame()).expect("failed to decode input");

    // Decode the WorkClaim<ReceiptClaim> containing the cycle count for this specific proof.
    let work_claim: WorkClaim<ReceiptClaim> =
        WorkClaim::deserialize_reader(&mut input.work_claim.as_slice())
            .expect("failed to decode WorkClaim");

    // Verify the work claim as a zkVM assumption. The host must have added the corresponding
    // receipt (with control root Digest::ZERO, the risc0-povw convention) to the ExecutorEnv.
    env::verify_assumption(work_claim.digest(), Digest::ZERO)
        .expect("work claim verification failed");

    // Derive the application proof's claim digest from the WorkClaim's embedded ReceiptClaim.
    // This cryptographically binds the cycle count to the specific application execution.
    let receipt_claim = work_claim.claim.as_value().expect("work claim.claim is pruned");
    let claim_digest: [u8; 32] = receipt_claim.digest().into();

    let update_value: u64 = work_claim.work.as_value().expect("work claim.work is pruned").value;

    let journal = MarketLogBuilderJournal {
        workLogId: input.log_id,
        updateValue: update_value,
        claimDigest: claim_digest.into(),
    };

    env::commit_slice(&journal.abi_encode());
}
