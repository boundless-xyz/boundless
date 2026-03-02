// Copyright 2026 Boundless Foundation, Inc.
//
// Use of this source code is governed by the Business Source License
// as found in the LICENSE-BSL file.

use alloy_primitives::{Address, B256, U160};
use alloy_sol_types::SolValue;
use boundless_povw::market_log_builder::{
    MarketLogBuilderInput, MarketLogBuilderJournal, BOUNDLESS_MARKET_LOG_BUILDER_ELF,
};
use boundless_test_utils::guests::ECHO_ELF;
use risc0_zkvm::{
    default_executor, default_prover, sha::Digestible, ExecutorEnv, ExitCode, ReceiptClaim,
    WorkClaim,
};

/// Verify the market-log-builder guest end-to-end.
#[test]
fn test_happy_path() {
    let log_id = Address::from([1u8; 20]);

    // 1. Prove the Echo guest with PoVW enabled. The dev-mode prover automatically produces
    //    prove_info.work_receipt containing the real ReceiptClaim and actual cycle count.
    let echo_env = ExecutorEnv::builder()
        .stdin(std::io::Cursor::new(b"hello world"))
        .povw((U160::from_be_bytes(log_id.0 .0), 0u64))
        .build()
        .unwrap();
    let prove_info = default_prover().prove(echo_env, ECHO_ELF).unwrap();
    let work_receipt =
        prove_info.work_receipt.expect("work_receipt missing; .povw() was not configured");

    // 2. Extract the WorkClaim — real ReceiptClaim binding + real cycle count, no manual values.
    let work_claim: WorkClaim<ReceiptClaim> =
        work_receipt.claim().value().expect("work_claim is pruned");
    let expected_cycles = work_claim.work.clone().value().expect("work.work is pruned").value;
    let expected_claim_digest = work_claim.claim.as_value().expect("claim is pruned").digest();

    // 3. Build market-log-builder input. No app_image_id or app_journal needed — the claim
    //    digest is derived from the WorkClaim's embedded ReceiptClaim inside the guest.
    let input = MarketLogBuilderInput { log_id, work_claim: borsh::to_vec(&work_claim).unwrap() }
        .encode()
        .unwrap();

    // 4. Execute. Only the work_receipt is needed as an assumption.
    let env =
        ExecutorEnv::builder().write_frame(&input).add_assumption(work_receipt).build().unwrap();
    let session = default_executor().execute(env, BOUNDLESS_MARKET_LOG_BUILDER_ELF).unwrap();
    assert_eq!(session.exit_code, ExitCode::Halted(0));

    // 5. Decode and assert the journal fields.
    let journal = MarketLogBuilderJournal::abi_decode(&session.journal.bytes).unwrap();
    assert_eq!(journal.workLogId, log_id);
    assert_eq!(journal.updateValue, expected_cycles);
    assert_eq!(journal.claimDigest, B256::from(<[u8; 32]>::from(expected_claim_digest)));
}

#[test]
#[should_panic]
fn test_wrong_claim_digest() {
    let log_id = Address::from([1u8; 20]);

    // 1. Prove the Echo guest with PoVW enabled. The dev-mode prover automatically produces
    //    prove_info.work_receipt containing the real ReceiptClaim and actual cycle count.
    let echo_env = ExecutorEnv::builder()
        .stdin(std::io::Cursor::new(b"hello world"))
        .povw((U160::from_be_bytes(log_id.0 .0), 0u64))
        .build()
        .unwrap();
    let prove_info = default_prover().prove(echo_env, ECHO_ELF).unwrap();
    let work_receipt =
        prove_info.work_receipt.expect("work_receipt missing; .povw() was not configured");

    // 2. Extract the WorkClaim — real ReceiptClaim binding + real cycle count, no manual values.
    let work_claim: WorkClaim<ReceiptClaim> =
        work_receipt.claim().value().expect("work_claim is pruned");
    let expected_cycles = work_claim.work.clone().value().expect("work.work is pruned").value;
    println!("expected_cycles 1: {}", expected_cycles);


    // Prove the Echo guest with PoVW enabled, but with a different input.
    let v = [1u8; 10000];
    let echo_env = ExecutorEnv::builder()
        .stdin(std::io::Cursor::new(v))
        .povw((U160::from_be_bytes(log_id.0 .0), 0u64))
        .build()
        .unwrap();
    let prove_info = default_prover().prove(echo_env, ECHO_ELF).unwrap();
    let work_receipt_2 =
        prove_info.work_receipt.expect("work_receipt missing; .povw() was not configured");

    let work_claim_2: WorkClaim<ReceiptClaim> =
        work_receipt_2.claim().value().expect("work_claim is pruned");
    let wrong_expected_cycles = work_claim_2.work.clone().value().expect("work.work is pruned").value;
    println!("wrong_expected_cycles: {}", wrong_expected_cycles);

    let wrong_work = work_claim_2.work.clone();
    let mut wrong_work_claim = work_claim.clone();
    wrong_work_claim.work = wrong_work;

    // 3. Build market-log-builder input. No app_image_id or app_journal needed — the claim
    //    digest is derived from the WorkClaim's embedded ReceiptClaim inside the guest.
    let input = MarketLogBuilderInput { log_id, work_claim: borsh::to_vec(&wrong_work_claim).unwrap() }
        .encode()
        .unwrap();

    // 4. Execute. Only the work_receipt is needed as an assumption.
    let env =
        ExecutorEnv::builder().write_frame(&input).add_assumption(work_receipt).build().unwrap();
    // should panic as the work claim is wrong
    default_executor().execute(env, BOUNDLESS_MARKET_LOG_BUILDER_ELF).unwrap();
}