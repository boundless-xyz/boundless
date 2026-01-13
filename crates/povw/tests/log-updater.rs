// Copyright 2026 Boundless Foundation, Inc.
//
// Use of this source code is governed by the Business Source License
// as found in the LICENSE-BSL file.

// NOTE: This test is fragile to changes in the RISC0_POVW_LOG_BUILDER_ID, because the ID used here
// and the one used in the log updater guest may drift. Running the following command may fix a
// drift if it appears.
//
// ```
// cargo update -p risc0-povw --manifest-path Cargo.toml && cargo update -p risc0-povw --manifest-path crates/povw/log-updater/Cargo.toml
// ```

use alloy::signers::local::PrivateKeySigner;
use alloy_primitives::{address, aliases::U96, Address, B256, U256};
use alloy_sol_types::SolValue;
use boundless_povw::log_updater::{
    Input, LogBuilderJournal, WorkLogUpdate, BOUNDLESS_POVW_LOG_UPDATER_ID,
};
use boundless_test_utils::povw::{execute_log_updater_guest, test_ctx};
use risc0_ethereum_contracts::encode_seal;
use risc0_povw::guest::RISC0_POVW_LOG_BUILDER_ID;
use risc0_povw::WorkLog;
use risc0_zkvm::{Digest, FakeReceipt, Receipt, ReceiptClaim};

#[tokio::test]
async fn basic() -> anyhow::Result<()> {
    let signer = PrivateKeySigner::random();
    let chain_id = 31337;
    let contract_address = address!("0x0000000000000000000000000000000000000f00");

    let update = LogBuilderJournal::builder()
        .self_image_id(RISC0_POVW_LOG_BUILDER_ID)
        .initial_commit(Digest::new(rand::random()))
        .updated_commit(Digest::new(rand::random()))
        .update_value(5)
        .work_log_id(signer.address())
        .build()?;

    let input = Input::builder()
        .update(update.clone())
        .contract_address(contract_address)
        .chain_id(chain_id)
        .sign_and_build(&signer)
        .await?;
    let journal = execute_log_updater_guest(&input)?;

    assert_eq!(journal.update.workLogId, signer.address());
    assert_eq!(journal.update.initialCommit, B256::from(<[u8; 32]>::from(update.initial_commit)));
    assert_eq!(journal.update.updatedCommit, B256::from(<[u8; 32]>::from(update.updated_commit)));
    assert_eq!(journal.update.updateValue, update.update_value);
    assert_eq!(journal.update.valueRecipient, signer.address());
    assert_eq!(
        journal.eip712Domain,
        WorkLogUpdate::eip712_domain(contract_address, chain_id).hash_struct()
    );
    Ok(())
}

#[tokio::test]
async fn reject_zero_value_recipient() -> anyhow::Result<()> {
    let signer = PrivateKeySigner::random();
    let chain_id = 31337;
    let contract_address = address!("0x0000000000000000000000000000000000000f00");

    let update = LogBuilderJournal::builder()
        .self_image_id(RISC0_POVW_LOG_BUILDER_ID)
        .initial_commit(Digest::new(rand::random()))
        .updated_commit(Digest::new(rand::random()))
        .update_value(5)
        .work_log_id(signer.address())
        .build()?;

    let input = Input::builder()
        .update(update.clone())
        .value_recipient(Address::ZERO)
        .contract_address(contract_address)
        .chain_id(chain_id)
        .sign_and_build(&signer)
        .await?;

    let err = execute_log_updater_guest(&input).unwrap_err();
    println!("execute_log_updater_guest failed with: {err}");
    assert!(err.to_string().contains("value recipient cannot be the zero address"));

    Ok(())
}

#[tokio::test]
async fn reject_wrong_signer() -> anyhow::Result<()> {
    let signer = PrivateKeySigner::random();
    let chain_id = 31337;
    let contract_address = address!("0x0000000000000000000000000000000000000f00");

    let update = LogBuilderJournal::builder()
        .self_image_id(RISC0_POVW_LOG_BUILDER_ID)
        .initial_commit(Digest::new(rand::random()))
        .updated_commit(Digest::new(rand::random()))
        .update_value(5)
        .work_log_id(signer.address())
        .build()?;

    let wrong_signer = PrivateKeySigner::random();
    let signature = WorkLogUpdate::from_log_builder_journal(update.clone(), signer.address())
        .sign(&wrong_signer, contract_address, chain_id)
        .await?;

    let input = Input::builder()
        .update(update.clone())
        .value_recipient(signer.address())
        .signature(signature.as_bytes().to_vec())
        .contract_address(contract_address)
        .chain_id(chain_id)
        .build()?;
    let err = execute_log_updater_guest(&input).unwrap_err();
    println!("execute_log_updater_guest failed with: {err}");
    assert!(err.to_string().contains("recovered signer does not match expected"));

    Ok(())
}

#[tokio::test]
async fn reject_wrong_chain_id() -> anyhow::Result<()> {
    let signer = PrivateKeySigner::random();
    let chain_id = 31337;
    let wrong_chain_id = 1; // Different chain ID
    let contract_address = address!("0x0000000000000000000000000000000000000f00");

    let update = LogBuilderJournal::builder()
        .self_image_id(RISC0_POVW_LOG_BUILDER_ID)
        .initial_commit(Digest::new(rand::random()))
        .updated_commit(Digest::new(rand::random()))
        .update_value(5)
        .work_log_id(signer.address())
        .build()?;

    let signature = WorkLogUpdate::from_log_builder_journal(update.clone(), signer.address())
        .sign(&signer, contract_address, wrong_chain_id)
        .await?;

    let input = Input::builder()
        .update(update.clone())
        .value_recipient(signer.address())
        .signature(signature.as_bytes().to_vec())
        .contract_address(contract_address)
        .chain_id(chain_id) // Correct chain ID in input, but signature was for wrong one
        .build()?;
    let err = execute_log_updater_guest(&input).unwrap_err();
    println!("execute_log_updater_guest failed with: {err}");
    assert!(err.to_string().contains("recovered signer does not match expected"));

    Ok(())
}

#[tokio::test]
async fn reject_wrong_chain_id_contract() -> anyhow::Result<()> {
    let ctx = test_ctx().await?;
    let signer = PrivateKeySigner::random();
    let wrong_chain_id = 1; // Different from Anvil's chain ID (31337)

    let update = LogBuilderJournal::builder()
        .self_image_id(RISC0_POVW_LOG_BUILDER_ID)
        .initial_commit(WorkLog::EMPTY.commit())
        .updated_commit(Digest::new(rand::random()))
        .update_value(10)
        .work_log_id(signer.address())
        .build()?;

    // Sign with wrong chain ID but execute with that same wrong chain ID
    let signature = WorkLogUpdate::from_log_builder_journal(update.clone(), signer.address())
        .sign(&signer, *ctx.povw_accounting.address(), wrong_chain_id)
        .await?;

    let input = Input::builder()
        .update(update.clone())
        .value_recipient(signer.address())
        .signature(signature.as_bytes().to_vec())
        .contract_address(*ctx.povw_accounting.address())
        .chain_id(wrong_chain_id) // Consistent but wrong chain ID
        .build()?;
    let journal = execute_log_updater_guest(&input)?;

    // Guest execution succeeds with wrong chain ID, but contract should reject
    let fake_receipt = risc0_zkvm::FakeReceipt::new(risc0_zkvm::ReceiptClaim::ok(
        BOUNDLESS_POVW_LOG_UPDATER_ID,
        journal.abi_encode(),
    ));
    let receipt: risc0_zkvm::Receipt = fake_receipt.try_into()?;

    let result = ctx
        .povw_accounting
        .updateWorkLog(
            journal.update.workLogId,
            journal.update.updatedCommit,
            journal.update.updateValue,
            journal.update.valueRecipient,
            encode_seal(&receipt)?.into(),
        )
        .send()
        .await;

    assert!(result.is_err(), "Contract should reject wrong chain ID");
    println!("Contract correctly rejected wrong chain ID: {:?}", result.unwrap_err());

    Ok(())
}

#[tokio::test]
async fn reject_wrong_contract_address() -> anyhow::Result<()> {
    let signer = PrivateKeySigner::random();
    let chain_id = 31337;
    let contract_address = address!("0x0000000000000000000000000000000000000f00");
    let wrong_contract_address = address!("0x0000000000000000000000000000000000000bad");

    let update = LogBuilderJournal::builder()
        .self_image_id(RISC0_POVW_LOG_BUILDER_ID)
        .initial_commit(Digest::new(rand::random()))
        .updated_commit(Digest::new(rand::random()))
        .update_value(5)
        .work_log_id(signer.address())
        .build()?;

    let signature = WorkLogUpdate::from_log_builder_journal(update.clone(), signer.address())
        .sign(&signer, wrong_contract_address, chain_id)
        .await?;

    let input = Input::builder()
        .update(update.clone())
        .value_recipient(signer.address())
        .signature(signature.as_bytes().to_vec())
        .contract_address(contract_address) // Correct contract address in input, but signature was for wrong one
        .chain_id(chain_id)
        .build()?;
    let err = execute_log_updater_guest(&input).unwrap_err();
    println!("execute_log_updater_guest failed with: {err}");
    assert!(err.to_string().contains("recovered signer does not match expected"));

    Ok(())
}

#[tokio::test]
async fn reject_invalid_initial_commit() -> anyhow::Result<()> {
    let ctx = test_ctx().await?;
    let signer = PrivateKeySigner::random();

    // First, post a valid update to establish a work log state
    let first_update = LogBuilderJournal::builder()
        .self_image_id(RISC0_POVW_LOG_BUILDER_ID)
        .initial_commit(WorkLog::EMPTY.commit())
        .updated_commit(Digest::new(rand::random()))
        .update_value(10)
        .work_log_id(signer.address())
        .build()?;

    let _first_event = ctx.post_work_log_update(&signer, &first_update, signer.address()).await?;

    // Now try to post a second update with wrong initial commit
    let wrong_initial_commit = Digest::new(rand::random()); // Should be first_update.updated_commit
    let second_update = LogBuilderJournal::builder()
        .self_image_id(RISC0_POVW_LOG_BUILDER_ID)
        .initial_commit(wrong_initial_commit)
        .updated_commit(Digest::new(rand::random()))
        .update_value(15)
        .work_log_id(signer.address())
        .build()?;

    // This should fail when posted to contract due to wrong initial commit
    let result = ctx.post_work_log_update(&signer, &second_update, signer.address()).await;
    assert!(result.is_err(), "Should reject invalid initial commit");

    let err = result.unwrap_err();
    println!("Contract correctly rejected invalid initial commit: {err}");
    // Check for verification failure selector 0x439cc0cd
    assert!(err.to_string().contains("0x439cc0cd"));

    Ok(())
}

#[tokio::test]
async fn reject_duplicate_update() -> anyhow::Result<()> {
    let ctx = test_ctx().await?;
    let signer = PrivateKeySigner::random();

    let update = LogBuilderJournal::builder()
        .self_image_id(RISC0_POVW_LOG_BUILDER_ID)
        .initial_commit(WorkLog::EMPTY.commit())
        .updated_commit(Digest::new(rand::random()))
        .update_value(10)
        .work_log_id(signer.address())
        .build()?;

    // Post the update successfully first time
    let _first_event = ctx.post_work_log_update(&signer, &update, signer.address()).await?;

    // Try to post the exact same update again - should fail
    let result = ctx.post_work_log_update(&signer, &update, signer.address()).await;
    assert!(result.is_err(), "Should reject duplicate update");

    let err = result.unwrap_err();
    println!("Contract correctly rejected duplicate update: {err}");
    // Check for verification failure selector 0x439cc0cd
    assert!(err.to_string().contains("0x439cc0cd"));

    Ok(())
}

#[tokio::test]
async fn reject_invalid_work_log_id() -> anyhow::Result<()> {
    let signer = PrivateKeySigner::random();
    let chain_id = 31337;
    let contract_address = address!("0x0000000000000000000000000000000000000f00");

    let update = LogBuilderJournal::builder()
        .self_image_id(RISC0_POVW_LOG_BUILDER_ID)
        .initial_commit(Digest::new(rand::random()))
        .updated_commit(Digest::new(rand::random()))
        .update_value(5)
        .work_log_id(Address::ZERO) // Invalid zero address
        .build()?;

    let signature = boundless_povw::log_updater::WorkLogUpdate::from_log_builder_journal(
        update.clone(),
        signer.address(),
    )
    .sign(&signer, contract_address, chain_id)
    .await?;

    let input = boundless_povw::log_updater::Input::builder()
        .update(update.clone())
        .value_recipient(signer.address())
        .signature(signature.as_bytes().to_vec())
        .contract_address(contract_address)
        .chain_id(chain_id)
        .build()?;
    let err = execute_log_updater_guest(&input).unwrap_err();
    println!("execute_log_updater_guest failed with: {err}");
    assert!(err.to_string().contains("recovered signer does not match expected"));

    Ok(())
}

#[tokio::test]
async fn reject_wrong_image_id() -> anyhow::Result<()> {
    let ctx = test_ctx().await?;
    let signer = PrivateKeySigner::random();

    let update = LogBuilderJournal::builder()
        .self_image_id(RISC0_POVW_LOG_BUILDER_ID)
        .initial_commit(WorkLog::EMPTY.commit())
        .updated_commit(Digest::new(rand::random()))
        .update_value(10)
        .work_log_id(signer.address())
        .build()?;

    // Execute guest to get valid journal
    let input = boundless_povw::log_updater::Input::builder()
        .update(update.clone())
        .contract_address(*ctx.povw_accounting.address())
        .chain_id(ctx.chain_id)
        .sign_and_build(&signer)
        .await?;

    let journal = execute_log_updater_guest(&input)?;

    // Create receipt with wrong image ID
    let wrong_image_id = risc0_zkvm::Digest::new([0xFFFFFFFFu32; 8]); // Wrong image ID
    let fake_receipt = risc0_zkvm::FakeReceipt::new(risc0_zkvm::ReceiptClaim::ok(
        wrong_image_id,
        journal.abi_encode(),
    ));
    let receipt: risc0_zkvm::Receipt = fake_receipt.try_into()?;

    // Try to submit to contract with wrong image ID - should fail
    let result = ctx
        .povw_accounting
        .updateWorkLog(
            journal.update.workLogId,
            journal.update.updatedCommit,
            journal.update.updateValue,
            journal.update.valueRecipient,
            encode_seal(&receipt)?.into(),
        )
        .send()
        .await;

    assert!(result.is_err(), "Contract should reject wrong image ID");
    let err = result.unwrap_err();
    println!("Contract correctly rejected wrong image ID: {err}");
    // Check for verification failure selector 0x439cc0cd
    assert!(err.to_string().contains("0x439cc0cd"));

    Ok(())
}

#[tokio::test]
async fn contract_integration() -> anyhow::Result<()> {
    let ctx = test_ctx().await?;

    let initial_epoch = ctx.zkc.getCurrentEpoch().call().await?;
    println!("Initial epoch: {initial_epoch}");

    // Construct and sign a WorkLogUpdate.
    let signer = PrivateKeySigner::random();
    let update = LogBuilderJournal::builder()
        .self_image_id(RISC0_POVW_LOG_BUILDER_ID)
        .initial_commit(WorkLog::EMPTY.commit())
        .updated_commit(Digest::new(rand::random()))
        .update_value(10)
        .work_log_id(signer.address())
        .build()?;

    let update_event = ctx.post_work_log_update(&signer, &update, signer.address()).await?;

    println!("WorkLogUpdated event: {update_event:?}");
    assert_eq!(update_event.workLogId, Address::from(update.work_log_id));
    assert_eq!(update_event.epochNumber, U256::from(initial_epoch));
    assert_eq!(update_event.initialCommit.as_slice(), update.initial_commit.as_bytes());
    assert_eq!(update_event.updatedCommit.as_slice(), update.updated_commit.as_bytes());
    assert_eq!(update_event.updateValue, U256::from(update.update_value));

    // Advance time to the next epoch and finalize the initial epoch.
    let new_epoch = ctx.advance_epochs(U256::ONE).await?;
    let finalized_event = ctx.finalize_epoch().await?;

    println!("EpochFinalized event: {finalized_event:?}");
    assert_eq!(finalized_event.epoch, U256::from(initial_epoch));
    assert_eq!(finalized_event.totalWork, U256::from(update.update_value));

    let pending_epoch = ctx.povw_accounting.pendingEpoch().call().await?;
    assert_eq!(pending_epoch.number, new_epoch);
    assert_eq!(pending_epoch.totalWork, U96::ZERO);

    Ok(())
}

#[tokio::test]
async fn two_updates_same_epoch_same_log_id() -> anyhow::Result<()> {
    let ctx = test_ctx().await?;

    let initial_epoch = ctx.zkc.getCurrentEpoch().call().await?;
    println!("Initial epoch: {initial_epoch}");

    let signer = PrivateKeySigner::random();

    // First update
    let first_update = LogBuilderJournal::builder()
        .self_image_id(RISC0_POVW_LOG_BUILDER_ID)
        .initial_commit(WorkLog::EMPTY.commit())
        .updated_commit(Digest::new(rand::random()))
        .update_value(10)
        .work_log_id(signer.address())
        .build()?;

    let first_event = ctx.post_work_log_update(&signer, &first_update, signer.address()).await?;
    println!("First WorkLogUpdated event: updateValue={}", first_event.updateValue);

    // Second update (chained from first)
    let second_update = LogBuilderJournal::builder()
        .self_image_id(RISC0_POVW_LOG_BUILDER_ID)
        .initial_commit(first_update.updated_commit) // Chain from first update
        .updated_commit(Digest::new(rand::random()))
        .update_value(15)
        .work_log_id(signer.address()) // Same log ID
        .build()?;

    let second_event = ctx.post_work_log_update(&signer, &second_update, signer.address()).await?;
    println!("Second WorkLogUpdated event: updateValue={}", second_event.updateValue);

    // Verify both events are in the same epoch
    assert_eq!(first_event.epochNumber, U256::from(initial_epoch));
    assert_eq!(second_event.epochNumber, U256::from(initial_epoch));
    assert_eq!(first_event.workLogId, second_event.workLogId);

    // Verify the commits chain correctly
    assert_eq!(first_event.updatedCommit, second_event.initialCommit);

    // Advance time and finalize epoch
    ctx.advance_epochs(U256::ONE).await?;
    let finalized_event = ctx.finalize_epoch().await?;

    // Total work should be sum of both updates
    assert_eq!(finalized_event.epoch, U256::from(initial_epoch));
    assert_eq!(finalized_event.totalWork, U256::from(25)); // 10 + 15
    Ok(())
}

#[tokio::test]
async fn two_updates_same_epoch_different_log_ids() -> anyhow::Result<()> {
    let ctx = test_ctx().await?;

    let initial_epoch = ctx.zkc.getCurrentEpoch().call().await?;
    println!("Initial epoch: {initial_epoch}");

    let signer1 = PrivateKeySigner::random();
    let signer2 = PrivateKeySigner::random();

    // First update with first log ID
    let first_update = LogBuilderJournal::builder()
        .self_image_id(RISC0_POVW_LOG_BUILDER_ID)
        .initial_commit(WorkLog::EMPTY.commit())
        .updated_commit(Digest::new(rand::random()))
        .update_value(20)
        .work_log_id(signer1.address())
        .build()?;

    let first_event = ctx.post_work_log_update(&signer1, &first_update, signer1.address()).await?;
    println!(
        "First WorkLogUpdated event: logId={}, updateValue={}",
        first_event.workLogId, first_event.updateValue
    );

    // Second update with different log ID
    let second_update = LogBuilderJournal::builder()
        .self_image_id(RISC0_POVW_LOG_BUILDER_ID)
        .initial_commit(WorkLog::EMPTY.commit())
        .updated_commit(Digest::new(rand::random()))
        .update_value(30)
        .work_log_id(signer2.address())
        .build()?;

    let second_event =
        ctx.post_work_log_update(&signer2, &second_update, signer2.address()).await?;
    println!(
        "Second WorkLogUpdated event: logId={}, updateValue={}",
        second_event.workLogId, second_event.updateValue
    );

    // Verify both events are in the same epoch but have different log IDs
    assert_eq!(first_event.epochNumber, U256::from(initial_epoch));
    assert_eq!(second_event.epochNumber, U256::from(initial_epoch));
    assert_ne!(first_event.workLogId, second_event.workLogId);

    // Both should start from empty commit since they're different logs
    assert_eq!(first_event.initialCommit.as_slice(), WorkLog::EMPTY.commit().as_bytes());
    assert_eq!(second_event.initialCommit.as_slice(), WorkLog::EMPTY.commit().as_bytes());

    // Advance time and finalize epoch
    ctx.advance_epochs(U256::ONE).await?;
    let finalized_event = ctx.finalize_epoch().await?;

    // Total work should be sum of both different log updates
    assert_eq!(finalized_event.epoch, U256::from(initial_epoch));
    assert_eq!(finalized_event.totalWork, U256::from(50)); // 20 + 30
    Ok(())
}

#[tokio::test]
async fn two_updates_subsequent_epochs_same_log_id() -> anyhow::Result<()> {
    let ctx = test_ctx().await?;

    let initial_epoch = ctx.zkc.getCurrentEpoch().call().await?;
    println!("Initial epoch: {initial_epoch}");

    let signer = PrivateKeySigner::random();

    // First update in first epoch
    let first_update = LogBuilderJournal::builder()
        .self_image_id(RISC0_POVW_LOG_BUILDER_ID)
        .initial_commit(WorkLog::EMPTY.commit())
        .updated_commit(Digest::new(rand::random()))
        .update_value(40)
        .work_log_id(signer.address())
        .build()?;

    let first_event = ctx.post_work_log_update(&signer, &first_update, signer.address()).await?;
    println!(
        "First WorkLogUpdated event in epoch {}: updateValue={}",
        first_event.epochNumber, first_event.updateValue
    );

    // Advance to next epoch and finalize the first epoch
    ctx.advance_epochs(U256::ONE).await?;
    let first_finalized_event = ctx.finalize_epoch().await?;

    let second_epoch = ctx.zkc.getCurrentEpoch().call().await?;
    println!("Advanced to epoch: {second_epoch}");

    // Verify first epoch was finalized correctly
    assert_eq!(first_finalized_event.epoch, U256::from(initial_epoch));
    assert_eq!(first_finalized_event.totalWork, U256::from(40));

    // Second update in second epoch (chained from first)
    let second_update = LogBuilderJournal::builder()
        .self_image_id(RISC0_POVW_LOG_BUILDER_ID)
        .initial_commit(first_update.updated_commit) // Chain from first update
        .updated_commit(Digest::new(rand::random()))
        .update_value(60)
        .work_log_id(signer.address()) // Same log ID
        .build()?;

    let second_event = ctx.post_work_log_update(&signer, &second_update, signer.address()).await?;
    println!(
        "Second WorkLogUpdated event in epoch {}: updateValue={}",
        second_event.epochNumber, second_event.updateValue
    );

    // Verify events are in different epochs with same log ID
    assert_eq!(first_event.epochNumber, U256::from(initial_epoch));
    assert_eq!(second_event.epochNumber, U256::from(second_epoch));
    assert_eq!(first_event.workLogId, second_event.workLogId);

    // Verify the commits chain correctly across epochs
    assert_eq!(first_event.updatedCommit, second_event.initialCommit);

    // Advance time and finalize second epoch
    ctx.advance_epochs(U256::ONE).await?;
    let second_finalized_event = ctx.finalize_epoch().await?;

    // Second epoch should only have work from second update
    assert_eq!(second_finalized_event.epoch, U256::from(second_epoch));
    assert_eq!(second_finalized_event.totalWork, U256::from(60));
    Ok(())
}

// Run log update transactions and measure the gas used. Ensure that it does not grow beyond a
// certain value without the developer realizing. Note that the reported gas values are using a
// mock receipt. Using a Groth16 or SetInclusion receipt will have higher gas costs.
#[tokio::test]
async fn measure_log_update_gas() -> anyhow::Result<()> {
    let ctx = test_ctx().await?;
    let signer = PrivateKeySigner::random();

    let measure_update_gas = {
        // Clone variables that we will need in the closure.
        let ctx = ctx.clone();
        let signer = signer.clone();

        async move |update: LogBuilderJournal| -> anyhow::Result<u64> {
            let input = Input::builder()
                .update(update.clone())
                .contract_address(*ctx.povw_accounting.address())
                .chain_id(ctx.chain_id)
                .sign_and_build(&signer)
                .await?;
            let journal = execute_log_updater_guest(&input)?;
            let fake_receipt: Receipt = FakeReceipt::new(ReceiptClaim::ok(
                BOUNDLESS_POVW_LOG_UPDATER_ID,
                journal.abi_encode(),
            ))
            .try_into()?;

            // Call the PovwAccounting.updateWorkLog function and confirm that it does not revert.
            let tx_result = ctx
                .povw_accounting
                .updateWorkLog(
                    journal.update.workLogId,
                    journal.update.updatedCommit,
                    journal.update.updateValue,
                    journal.update.valueRecipient,
                    encode_seal(&fake_receipt)?.into(),
                )
                .send()
                .await?;

            let receipt = tx_result.get_receipt().await?;
            println!("Gas used for tx {}: {}", receipt.transaction_hash, receipt.gas_used);

            Ok(receipt.gas_used)
        }
    };

    let first_update = LogBuilderJournal::builder()
        .self_image_id(RISC0_POVW_LOG_BUILDER_ID)
        .initial_commit(WorkLog::EMPTY.commit())
        .updated_commit(Digest::new(rand::random()))
        .update_value(40)
        .work_log_id(signer.address())
        .build()?;

    // Second update in second epoch (chained from first)
    let second_update = LogBuilderJournal::builder()
        .self_image_id(RISC0_POVW_LOG_BUILDER_ID)
        .initial_commit(first_update.updated_commit)
        .updated_commit(Digest::new(rand::random()))
        .update_value(60)
        .work_log_id(signer.address())
        .build()?;

    let third_update = LogBuilderJournal::builder()
        .self_image_id(RISC0_POVW_LOG_BUILDER_ID)
        .initial_commit(second_update.updated_commit)
        .updated_commit(Digest::new(rand::random()))
        .update_value(20)
        .work_log_id(signer.address())
        .build()?;

    // First update: from the initial state.
    assert!(measure_update_gas(first_update).await? < 80000);

    // First update: from the state of a fresh epoch. Finalizes the epoch.
    ctx.advance_epochs(U256::ONE).await?;
    assert!(measure_update_gas(second_update).await? < 65000);

    // Second update within the same epoch.
    assert!(measure_update_gas(third_update).await? < 60000);

    Ok(())
}

#[tokio::test]
async fn separate_value_recipient() -> anyhow::Result<()> {
    let ctx = test_ctx().await?;
    let work_log_signer = PrivateKeySigner::random();
    let value_recipient = PrivateKeySigner::random();

    let initial_epoch = ctx.zkc.getCurrentEpoch().call().await?;
    println!("Initial epoch: {initial_epoch}");

    // Work log is controlled by work_log_signer but rewards go to value_recipient
    let update = LogBuilderJournal::builder()
        .self_image_id(RISC0_POVW_LOG_BUILDER_ID)
        .initial_commit(WorkLog::EMPTY.commit())
        .updated_commit(Digest::new(rand::random()))
        .update_value(25)
        .work_log_id(work_log_signer.address())
        .build()?;

    let update_event =
        ctx.post_work_log_update(&work_log_signer, &update, value_recipient.address()).await?;
    println!("WorkLogUpdated event: {update_event:?}");

    // Verify event fields
    assert_eq!(update_event.workLogId, Address::from(update.work_log_id));
    assert_eq!(update_event.valueRecipient, value_recipient.address());
    assert_eq!(update_event.updateValue, U256::from(update.update_value));

    Ok(())
}

#[tokio::test]
async fn multiple_recipients_same_work_log() -> anyhow::Result<()> {
    let ctx = test_ctx().await?;
    let work_log_signer = PrivateKeySigner::random();
    let recipient1 = PrivateKeySigner::random();
    let recipient2 = PrivateKeySigner::random();

    let initial_epoch = ctx.zkc.getCurrentEpoch().call().await?;
    println!("Initial epoch: {initial_epoch}");

    // First update: same work log, recipient1 gets rewards
    let first_update = LogBuilderJournal::builder()
        .self_image_id(RISC0_POVW_LOG_BUILDER_ID)
        .initial_commit(WorkLog::EMPTY.commit())
        .updated_commit(Digest::new(rand::random()))
        .update_value(30)
        .work_log_id(work_log_signer.address())
        .build()?;

    let first_event =
        ctx.post_work_log_update(&work_log_signer, &first_update, recipient1.address()).await?;
    println!("First update: recipient1 gets {} work units", first_event.updateValue);

    // Second update: same work log (chained), recipient2 gets rewards
    let second_update = LogBuilderJournal::builder()
        .self_image_id(RISC0_POVW_LOG_BUILDER_ID)
        .initial_commit(first_update.updated_commit)
        .updated_commit(Digest::new(rand::random()))
        .update_value(20)
        .work_log_id(work_log_signer.address())
        .build()?;

    let second_event =
        ctx.post_work_log_update(&work_log_signer, &second_update, recipient2.address()).await?;
    println!("Second update: recipient2 gets {} work units", second_event.updateValue);

    // Verify both updates have same work log ID but different recipients
    assert_eq!(first_event.workLogId, second_event.workLogId);
    assert_ne!(first_event.valueRecipient, second_event.valueRecipient);
    assert_eq!(first_event.valueRecipient, recipient1.address());
    assert_eq!(second_event.valueRecipient, recipient2.address());

    // Verify commits chain correctly
    assert_eq!(first_event.updatedCommit, second_event.initialCommit);

    Ok(())
}
