// Copyright (c) 2025 RISC Zero, Inc.
//
// All rights reserved.

//! Integration test demonstrating the full PoVW proving pipeline from work receipts
//! to smart contract updates using both WorkLogUpdateProver and LogUpdaterProver.

mod common;

use alloy::signers::local::PrivateKeySigner;
use alloy_sol_types::SolValue;
use boundless_povw_guests::log_updater::{prover::LogUpdaterProver, Journal as LogUpdaterJournal};
use risc0_povw::{prover::WorkLogUpdateProver, PovwLogId};
use risc0_zkvm::{default_prover, FakeReceipt, ProverOpts, VerifierContext};

use common::make_work_claim;

#[tokio::test]
async fn test_workflow() -> anyhow::Result<()> {
    // Setup test context with smart contracts
    let ctx = common::test_ctx().await?;
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
        .contract_address(*ctx.povw_accounting_contract.address())
        .chain_id(ctx.chain_id)
        .prover_opts(ProverOpts::default().with_dev_mode(true))
        .verifier_ctx(VerifierContext::default().with_dev_mode(true))
        .build()?;

    let log_updater_prove_info =
        log_updater_prover.prove_update(log_builder_receipt, &signer).await?;

    // TODO(povw): Provide a more concise way to send the log update.
    // Step 4: Decode the journal and post to the smart contract
    let journal = LogUpdaterJournal::abi_decode(
        &log_updater_prove_info.receipt.journal.bytes,
    )?;

    // Verify the journal contains expected values
    assert_eq!(journal.update.workLogId, signer.address());
    assert_eq!(journal.update.updateValue, 1 << 20); // 1M value from work
    assert_eq!(journal.update.valueRecipient, signer.address());

    // Encode the receipt for the smart contract
    let seal = common::encode_seal(&log_updater_prove_info.receipt)?;

    // Call the PovwAccounting.updateWorkLog function
    let tx_result = ctx
        .povw_accounting_contract
        .updateWorkLog(
            journal.update.workLogId,
            journal.update.updatedCommit,
            journal.update.updateValue,
            journal.update.valueRecipient,
            seal.into(),
        )
        .send()
        .await?;

    // Verify the transaction succeeded
    let receipt = tx_result.get_receipt().await?;
    assert!(receipt.status());

    // Query for the expected WorkLogUpdated event
    let logs = receipt.logs();
    let work_log_updated_events = logs
        .iter()
        .filter_map(|log| {
            log.log_decode::<boundless_povw_guests::log_updater::IPovwAccounting::WorkLogUpdated>()
                .ok()
        })
        .collect::<Vec<_>>();

    assert_eq!(work_log_updated_events.len(), 1, "Expected exactly one WorkLogUpdated event");
    let event = &work_log_updated_events[0].inner.data;
    assert_eq!(event.workLogId, signer.address());
    assert_eq!(event.updateValue, 1 << 20);
    assert_eq!(event.valueRecipient, signer.address());

    Ok(())
}
