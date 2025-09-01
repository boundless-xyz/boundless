// Copyright (c) 2025 RISC Zero, Inc.
//
// All rights reserved.

//! Integration test demonstrating the full PoVW proving pipeline from work receipts
//! to smart contract updates using WorkLogUpdateProver, LogUpdaterProver, and MintCalculatorProver.

use alloy::{primitives::U256, signers::local::PrivateKeySigner};
use boundless_povw_guests::{
    log_updater::{prover::LogUpdaterProver, IPovwAccounting},
    mint_calculator::{prover::MintCalculatorProver, WorkLogFilter},
};
use boundless_test_utils::povw::{make_work_claim, test_ctx};
use risc0_povw::{prover::WorkLogUpdateProver, PovwLogId};
use risc0_steel::ethereum::ANVIL_CHAIN_SPEC;
use risc0_zkvm::{default_prover, FakeReceipt, ProverOpts, VerifierContext};

#[tokio::test]
async fn test_workflow() -> anyhow::Result<()> {
    // Setup test context with smart contracts
    let ctx = test_ctx().await?;
    let signer = PrivateKeySigner::random();
    let log_id: PovwLogId = signer.address().into();

    // Step 1: Create a fake work receipt
    let work_claim = make_work_claim((log_id, 42), 11, 1 << 20)?; // 11 segments, 1M value
    let work_receipt = FakeReceipt::new(work_claim).into();

    // Step 2: Use WorkLogUpdateProver to create a Log Builder receipt
    let mut work_log_prover = WorkLogUpdateProver::builder()
        .prover(default_prover())
        .log_id(log_id)
        .prover_opts(ProverOpts::default().with_dev_mode(true))
        .verifier_ctx(VerifierContext::default().with_dev_mode(true))
        .build()?;

    let log_builder_prove_info = work_log_prover.prove_update([work_receipt])?;
    let log_builder_receipt = log_builder_prove_info.receipt;

    // Step 3: Use LogUpdaterProver to create a Log Updater receipt
    let log_updater_prover = LogUpdaterProver::builder()
        .prover(default_prover())
        .contract_address(*ctx.povw_accounting.address())
        .chain_id(ctx.chain_id)
        .prover_opts(ProverOpts::default().with_dev_mode(true))
        .verifier_ctx(VerifierContext::default().with_dev_mode(true))
        .build()?;

    let log_updater_prove_info =
        log_updater_prover.prove_update(log_builder_receipt, &signer).await?;

    // Step 4: Post the proven log update to the smart contract
    let tx_receipt = ctx
        .povw_accounting
        .update_work_log(&log_updater_prove_info.receipt)?
        .send()
        .await?
        .get_receipt()
        .await?;

    assert!(tx_receipt.status());

    // Query for the expected WorkLogUpdated event
    let logs = tx_receipt.logs();
    let work_log_updated_events = logs
        .iter()
        .filter_map(|log| log.log_decode::<IPovwAccounting::WorkLogUpdated>().ok())
        .collect::<Vec<_>>();

    assert_eq!(work_log_updated_events.len(), 1, "Expected exactly one WorkLogUpdated event");
    let event = &work_log_updated_events[0].inner.data;
    assert_eq!(event.workLogId, signer.address());
    assert_eq!(event.updateValue, 1 << 20);
    assert_eq!(event.valueRecipient, signer.address());

    Ok(())
}
