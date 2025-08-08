// Copyright (c) 2025 RISC Zero, Inc.
//
// All rights reserved.


use alloy::{
    primitives::{B256, U256},
    signers::local::PrivateKeySigner,
};
use alloy_sol_types::SolValue;

use boundless_povw_guests::{
    log_updater::LogBuilderJournal,
    mint_calculator::{
        FixedPoint, MintCalculatorJournal, MintCalculatorMint,
        MintCalculatorUpdate,
    }, BOUNDLESS_POVW_MINT_CALCULATOR_ID,
};
use risc0_povw::WorkLog;
use risc0_povw_guests::RISC0_POVW_LOG_BUILDER_ID;
use risc0_steel::ethereum::ETH_SEPOLIA_CHAIN_SPEC;
use risc0_zkvm::{Digest, FakeReceipt, Receipt, ReceiptClaim};

mod common;

#[tokio::test]
async fn basic() -> anyhow::Result<()> {
    // Setup test context
    let ctx = common::text_ctx().await?;

    let initial_epoch = ctx.povw_contract.currentEpoch().call().await?;
    println!("Initial epoch: {initial_epoch}");

    // Post a work log update
    let signer = PrivateKeySigner::random();
    let update = LogBuilderJournal {
        self_image_id: RISC0_POVW_LOG_BUILDER_ID.into(),
        initial_commit: WorkLog::EMPTY.commit(),
        updated_commit: Digest::new(rand::random()),
        update_value: 25, // Work value for this update
        work_log_id: signer.address().into(),
    };

    let work_log_event = ctx.post_work_log_update(&signer, &update, signer.address()).await?;
    println!("Work log update posted for epoch {}", work_log_event.epochNumber);

    // Advance time and finalize epoch
    ctx.advance_epochs(1).await?;
    let finalized_event = ctx.finalize_epoch().await?;

    assert_eq!(finalized_event.epoch, U256::from(initial_epoch));
    assert_eq!(finalized_event.totalWork, U256::from(25)); // Our work log update value
    println!(
        "EpochFinalized event verified: epoch={}, totalWork={}",
        finalized_event.epoch, finalized_event.totalWork
    );

    let mint_receipt = ctx.run_mint().await?;
    println!("Mint transaction succeeded with {} gas used", mint_receipt.gas_used);

    let final_balance = ctx.token_contract.balanceOf(signer.address()).call().await?;
    let epoch_reward = ctx.mint_contract.EPOCH_REWARD().call().await?;

    assert_eq!(final_balance, epoch_reward, "Minted amount should match expected calculation");
    Ok(())
}

#[tokio::test]
async fn proportional_rewards_same_epoch() -> anyhow::Result<()> {
    let ctx = common::text_ctx().await?;

    let initial_epoch = ctx.povw_contract.currentEpoch().call().await?;
    println!("Initial epoch: {initial_epoch}");

    let signer1 = PrivateKeySigner::random();
    let signer2 = PrivateKeySigner::random();

    // First update: 30 work units
    let update1 = LogBuilderJournal {
        self_image_id: RISC0_POVW_LOG_BUILDER_ID.into(),
        initial_commit: WorkLog::EMPTY.commit(),
        updated_commit: Digest::new(rand::random()),
        update_value: 30,
        work_log_id: signer1.address().into(),
    };

    // Second update: 70 work units (different log ID)
    let update2 = LogBuilderJournal {
        self_image_id: RISC0_POVW_LOG_BUILDER_ID.into(),
        initial_commit: WorkLog::EMPTY.commit(),
        updated_commit: Digest::new(rand::random()),
        update_value: 70,
        work_log_id: signer2.address().into(),
    };

    let event1 = ctx.post_work_log_update(&signer1, &update1, signer1.address()).await?;
    let event2 = ctx.post_work_log_update(&signer2, &update2, signer2.address()).await?;

    println!("Update 1: {} work units for {:?}", event1.updateValue, event1.workLogId);
    println!("Update 2: {} work units for {:?}", event2.updateValue, event2.workLogId);

    // Advance time and finalize epoch
    ctx.advance_epochs(1).await?;
    let finalized_event = ctx.finalize_epoch().await?;

    // Total work should be 30 + 70 = 100
    assert_eq!(finalized_event.totalWork, U256::from(100));
    println!("Total work in epoch: {}", finalized_event.totalWork);

    // Run mint calculation
    let mint_receipt = ctx.run_mint().await?;
    println!("Mint transaction succeeded with {} gas used", mint_receipt.gas_used);

    // Check balances - should be proportional to work done
    let balance1 = ctx.token_contract.balanceOf(signer1.address()).call().await?;
    let balance2 = ctx.token_contract.balanceOf(signer2.address()).call().await?;
    let epoch_reward = ctx.mint_contract.EPOCH_REWARD().call().await?;

    // Expected: signer1 gets 30%, signer2 gets 70%
    let expected1 = epoch_reward * U256::from(30) / U256::from(100);
    let expected2 = epoch_reward * U256::from(70) / U256::from(100);

    // Allow for small rounding errors in fixed-point arithmetic (within 10 wei)
    // TODO(povw): Try to avoid these rounding errors.
    let tolerance = U256::from(10);
    assert!(
        balance1.abs_diff(expected1) <= tolerance,
        "Signer1 should receive ~30 tokens, got {balance1}, expected {expected1}"
    );
    assert!(
        balance2.abs_diff(expected2) <= tolerance,
        "Signer2 should receive ~70 tokens, got {balance2}, expected {expected2}"
    );

    println!(
        "Proportional rewards verified: {balance1} tokens to signer1, {balance2} tokens to signer2"
    );
    Ok(())
}

#[tokio::test]
async fn sequential_mints_per_epoch() -> anyhow::Result<()> {
    let ctx = common::text_ctx().await?;

    let first_epoch = ctx.povw_contract.currentEpoch().call().await?;
    println!("Starting epoch: {first_epoch}");

    let signer = PrivateKeySigner::random();

    // First epoch update
    let update1 = LogBuilderJournal {
        self_image_id: RISC0_POVW_LOG_BUILDER_ID.into(),
        initial_commit: WorkLog::EMPTY.commit(),
        updated_commit: Digest::new(rand::random()),
        update_value: 50,
        work_log_id: signer.address().into(),
    };

    let event1 = ctx.post_work_log_update(&signer, &update1, signer.address()).await?;
    println!("Update 1: {} work units in epoch {}", event1.updateValue, event1.epochNumber);

    // Advance to next epoch and finalize first epoch
    ctx.advance_epochs(1).await?;
    let finalized_event1 = ctx.finalize_epoch().await?;
    assert_eq!(finalized_event1.totalWork, U256::from(50));

    // First mint for first epoch
    let mint_receipt1 = ctx.run_mint_for_epochs(&[first_epoch]).await?;
    println!("First mint completed with {} gas used", mint_receipt1.gas_used);

    let balance_after_first_mint = ctx.token_contract.balanceOf(signer.address()).call().await?;
    let epoch_reward = ctx.mint_contract.EPOCH_REWARD().call().await?;

    assert_eq!(
        balance_after_first_mint, epoch_reward,
        "After first mint should have full epoch reward"
    );
    println!("Balance after first mint: {balance_after_first_mint} tokens");

    // Second epoch update (chained from first)
    let second_epoch = ctx.povw_contract.currentEpoch().call().await?;
    let update2 = LogBuilderJournal {
        self_image_id: RISC0_POVW_LOG_BUILDER_ID.into(),
        initial_commit: update1.updated_commit, // Chain from first update
        updated_commit: Digest::new(rand::random()),
        update_value: 75,
        work_log_id: signer.address().into(),
    };

    let event2 = ctx.post_work_log_update(&signer, &update2, signer.address()).await?;
    println!("Update 2: {} work units in epoch {}", event2.updateValue, event2.epochNumber);

    // Advance to next epoch and finalize second epoch
    ctx.advance_epochs(1).await?;
    let finalized_event2 = ctx.finalize_epoch().await?;
    assert_eq!(finalized_event2.epoch, U256::from(second_epoch));
    assert_eq!(finalized_event2.totalWork, U256::from(75));

    // Second mint for second epoch
    let mint_receipt2 = ctx.run_mint_for_epochs(&[second_epoch]).await?;
    println!("Second mint completed with {} gas used", mint_receipt2.gas_used);

    let final_balance = ctx.token_contract.balanceOf(signer.address()).call().await?;
    let expected_total = epoch_reward * U256::from(2); // Both full epoch rewards

    assert_eq!(final_balance, expected_total, "Final balance should be exactly 2x epoch reward");
    println!("Final balance after both mints: {final_balance} tokens");

    Ok(())
}

#[tokio::test]
async fn cross_epoch_mint() -> anyhow::Result<()> {
    let ctx = common::text_ctx().await?;

    let first_epoch = ctx.povw_contract.currentEpoch().call().await?;
    println!("Starting epoch: {first_epoch}");

    let signer = PrivateKeySigner::random();

    // First epoch update
    let update1 = LogBuilderJournal {
        self_image_id: RISC0_POVW_LOG_BUILDER_ID.into(),
        initial_commit: WorkLog::EMPTY.commit(),
        updated_commit: Digest::new(rand::random()),
        update_value: 40,
        work_log_id: signer.address().into(),
    };

    let event1 = ctx.post_work_log_update(&signer, &update1, signer.address()).await?;
    println!("Update 1: {} work units in epoch {}", event1.updateValue, event1.epochNumber);

    // Advance to next epoch and finalize first epoch
    ctx.advance_epochs(1).await?;
    let finalized_event1 = ctx.finalize_epoch().await?;
    assert_eq!(finalized_event1.totalWork, U256::from(40));

    // Second epoch update (chained from first)
    let second_epoch = ctx.povw_contract.currentEpoch().call().await?;
    let update2 = LogBuilderJournal {
        self_image_id: RISC0_POVW_LOG_BUILDER_ID.into(),
        initial_commit: update1.updated_commit, // Chain from first update
        updated_commit: Digest::new(rand::random()),
        update_value: 60,
        work_log_id: signer.address().into(),
    };

    let event2 = ctx.post_work_log_update(&signer, &update2, signer.address()).await?;
    println!("Update 2: {} work units in epoch {}", event2.updateValue, event2.epochNumber);

    // Advance to next epoch and finalize second epoch
    ctx.advance_epochs(1).await?;
    let finalized_event2 = ctx.finalize_epoch().await?;
    assert_eq!(finalized_event2.epoch, U256::from(second_epoch));
    assert_eq!(finalized_event2.totalWork, U256::from(60));

    // Single mint covering both epochs
    let mint_receipt = ctx.run_mint_for_epochs(&[first_epoch, second_epoch]).await?;
    println!("Cross-epoch mint completed with {} gas used", mint_receipt.gas_used);

    let final_balance = ctx.token_contract.balanceOf(signer.address()).call().await?;
    let epoch_reward = ctx.mint_contract.EPOCH_REWARD().call().await?;
    let expected_total = epoch_reward * U256::from(2); // Both full epoch rewards

    assert_eq!(
        final_balance, expected_total,
        "Final balance should be exactly 2x epoch reward from both epochs"
    );
    println!("Final balance after cross-epoch mint: {final_balance} tokens");

    Ok(())
}

#[tokio::test]
async fn reject_invalid_steel_commitment() -> anyhow::Result<()> {
    let ctx = common::text_ctx().await?;
    let signer = PrivateKeySigner::random();

    // Setup a basic work log and epoch
    let update = LogBuilderJournal {
        self_image_id: RISC0_POVW_LOG_BUILDER_ID.into(),
        initial_commit: WorkLog::EMPTY.commit(),
        updated_commit: Digest::new(rand::random()),
        update_value: 25,
        work_log_id: signer.address().into(),
    };

    let _work_log_event = ctx.post_work_log_update(&signer, &update, signer.address()).await?;
    ctx.advance_epochs(1).await?;
    let _finalized_event = ctx.finalize_epoch().await?;

    // Create a mint journal with invalid Steel commitment
    let mint_journal = MintCalculatorJournal {
        mints: vec![MintCalculatorMint {
            recipient: signer.address(),
            value: FixedPoint { value: U256::from(1) },
        }],
        updates: vec![MintCalculatorUpdate {
            workLogId: signer.address(),
            initialCommit: B256::from(<[u8; 32]>::from(update.initial_commit)),
            updatedCommit: B256::from(<[u8; 32]>::from(update.updated_commit)),
        }],
        povwContractAddress: *ctx.povw_contract.address(),
        steelCommit: risc0_steel::Commitment::default(), // Invalid/empty Steel commitment
    };

    // Create fake receipt and try to submit
    let fake_receipt = FakeReceipt::new(ReceiptClaim::ok(
        BOUNDLESS_POVW_MINT_CALCULATOR_ID,
        mint_journal.abi_encode(),
    ));
    let receipt: Receipt = fake_receipt.try_into()?;

    let result = ctx
        .mint_contract
        .mint(mint_journal.abi_encode().into(), common::encode_seal(&receipt)?.into())
        .send()
        .await;

    assert!(result.is_err(), "Should reject invalid Steel commitment");
    let err = result.unwrap_err();
    println!("Contract correctly rejected invalid Steel commitment: {err}");
    // Check for InvalidSteelCommitment error selector 0xa7e6de3e
    assert!(err.to_string().contains("0xa7e6de3e"));

    Ok(())
}

#[tokio::test]
async fn reject_wrong_povw_address() -> anyhow::Result<()> {
    let ctx1 = common::text_ctx().await?;
    let ctx2 = common::test_ctx_with(ctx1.anvil.clone(), 1).await?;

    let signer = PrivateKeySigner::random();
    let update = LogBuilderJournal {
        self_image_id: RISC0_POVW_LOG_BUILDER_ID.into(),
        initial_commit: WorkLog::EMPTY.commit(),
        updated_commit: Digest::new(rand::random()),
        update_value: 25, // Work value for this update
        work_log_id: signer.address().into(),
    };

    // Using deployment #1, build the mint inputs.
    ctx1.post_work_log_update(&signer, &update, signer.address()).await?;
    ctx1.advance_epochs(1).await?;
    ctx1.finalize_epoch().await?;

    let mint_input = ctx1.build_mint_input().await?;

    // Execute the mint calculator guest
    let mint_journal = common::execute_mint_calculator_guest(&mint_input)?;

    // Assemble a fake receipt and use it to call the mint function on the PovwMint contract.
    let mint_receipt: Receipt = FakeReceipt::new(ReceiptClaim::ok(
        BOUNDLESS_POVW_MINT_CALCULATOR_ID,
        mint_journal.abi_encode(),
    ))
    .try_into()?;

    // Submit the mint to deployment #2. This should fail as the contract address for the PovwAccounting
    // contract is wrong.
    let result = ctx2
        .mint_contract
        .mint(mint_journal.abi_encode().into(), common::encode_seal(&mint_receipt)?.into())
        .send()
        .await;

    assert!(result.is_err(), "Should reject wrong PovwAccounting contract address");
    let err = result.unwrap_err();
    println!("Contract correctly rejected wrong PovwAccounting address: {err}");
    // Check for IncorrectPovwAddress error selector 0x82db2de2
    assert!(err.to_string().contains("0x82db2de2"));

    Ok(())
}

#[tokio::test]
async fn reject_mint_with_only_latter_epoch() -> anyhow::Result<()> {
    let ctx = common::text_ctx().await?;
    let signer = PrivateKeySigner::random();

    let _first_epoch = ctx.povw_contract.currentEpoch().call().await?;

    // First update in first epoch
    let update1 = LogBuilderJournal {
        self_image_id: RISC0_POVW_LOG_BUILDER_ID.into(),
        initial_commit: WorkLog::EMPTY.commit(),
        updated_commit: Digest::new(rand::random()),
        update_value: 30,
        work_log_id: signer.address().into(),
    };

    ctx.post_work_log_update(&signer, &update1, signer.address()).await?;
    ctx.advance_epochs(1).await?;
    ctx.finalize_epoch().await?;

    let second_epoch = ctx.povw_contract.currentEpoch().call().await?;

    // Second update in second epoch (chained from first)
    let update2 = LogBuilderJournal {
        self_image_id: RISC0_POVW_LOG_BUILDER_ID.into(),
        initial_commit: update1.updated_commit,
        updated_commit: Digest::new(rand::random()),
        update_value: 40,
        work_log_id: signer.address().into(),
    };

    ctx.post_work_log_update(&signer, &update2, signer.address()).await?;
    ctx.advance_epochs(1).await?;
    ctx.finalize_epoch().await?;

    // Try to mint using only the second epoch - should fail
    let result = ctx.run_mint_for_epochs(&[second_epoch]).await;
    assert!(result.is_err(), "Should reject mint with incomplete chain");
    let err = result.unwrap_err();
    println!("Contract correctly rejected incomplete chain: {err}");
    // Check for IncorrectInitialUpdateCommit error selector 0xf4a2b615
    assert!(err.to_string().contains("0xf4a2b615"));

    Ok(())
}

#[tokio::test]
async fn reject_mint_with_skipped_epoch() -> anyhow::Result<()> {
    let ctx = common::text_ctx().await?;
    let signer = PrivateKeySigner::random();

    let first_epoch = ctx.povw_contract.currentEpoch().call().await?;

    // First update in first epoch
    let update1 = LogBuilderJournal {
        self_image_id: RISC0_POVW_LOG_BUILDER_ID.into(),
        initial_commit: WorkLog::EMPTY.commit(),
        updated_commit: Digest::new(rand::random()),
        update_value: 20,
        work_log_id: signer.address().into(),
    };

    ctx.post_work_log_update(&signer, &update1, signer.address()).await?;
    ctx.advance_epochs(1).await?;
    ctx.finalize_epoch().await?;

    let _second_epoch = ctx.povw_contract.currentEpoch().call().await?;

    // Second update in second epoch (chained from first)
    let update2 = LogBuilderJournal {
        self_image_id: RISC0_POVW_LOG_BUILDER_ID.into(),
        initial_commit: update1.updated_commit,
        updated_commit: Digest::new(rand::random()),
        update_value: 30,
        work_log_id: signer.address().into(),
    };

    ctx.post_work_log_update(&signer, &update2, signer.address()).await?;
    ctx.advance_epochs(1).await?;
    ctx.finalize_epoch().await?;

    let third_epoch = ctx.povw_contract.currentEpoch().call().await?;

    // Third update in third epoch (chained from second)
    let update3 = LogBuilderJournal {
        self_image_id: RISC0_POVW_LOG_BUILDER_ID.into(),
        initial_commit: update2.updated_commit,
        updated_commit: Digest::new(rand::random()),
        update_value: 50,
        work_log_id: signer.address().into(),
    };

    ctx.post_work_log_update(&signer, &update3, signer.address()).await?;
    ctx.advance_epochs(1).await?;
    ctx.finalize_epoch().await?;

    // Try to mint using first and third epochs (skipping second) - should fail
    let result = ctx.run_mint_for_epochs(&[first_epoch, third_epoch]).await;
    assert!(result.is_err(), "Should reject mint with skipped epoch");
    let err = result.unwrap_err();
    println!("Contract correctly rejected skipped epoch: {err}");
    // Check for guest panic about non-chaining updates
    assert!(
        err.to_string().contains("multiple update events")
            && err.to_string().contains("do not form a chain")
    );

    Ok(())
}

#[tokio::test]
async fn reject_mint_with_unfinalized_epoch() -> anyhow::Result<()> {
    let ctx = common::text_ctx().await?;
    let signer = PrivateKeySigner::random();

    let current_epoch = ctx.povw_contract.currentEpoch().call().await?;

    // Post work log update but don't finalize the epoch
    let update = LogBuilderJournal {
        self_image_id: RISC0_POVW_LOG_BUILDER_ID.into(),
        initial_commit: WorkLog::EMPTY.commit(),
        updated_commit: Digest::new(rand::random()),
        update_value: 25,
        work_log_id: signer.address().into(),
    };

    ctx.post_work_log_update(&signer, &update, signer.address()).await?;

    // Advance time but DO NOT finalize the epoch
    ctx.advance_epochs(1).await?;

    // Try to mint without finalizing the epoch - should fail
    let result = ctx.run_mint_for_epochs(&[current_epoch]).await;
    assert!(result.is_err(), "Should reject mint with unfinalized epoch");
    let err = result.unwrap_err();
    println!("Contract correctly rejected unfinalized epoch: {err}");
    // The mint calculator guest should fail because there's no EpochFinalized event
    assert!(err.to_string().contains("no epoch finalized event processed"));

    Ok(())
}

#[tokio::test]
async fn reject_mint_wrong_chain_spec() -> anyhow::Result<()> {
    // Setup test context
    let ctx = common::text_ctx().await?;

    // Post a work log update
    let signer = PrivateKeySigner::random();
    let update = LogBuilderJournal {
        self_image_id: RISC0_POVW_LOG_BUILDER_ID.into(),
        initial_commit: WorkLog::EMPTY.commit(),
        updated_commit: Digest::new(rand::random()),
        update_value: 25, // Work value for this update
        work_log_id: signer.address().into(),
    };

    ctx.post_work_log_update(&signer, &update, signer.address()).await?;

    // Advance time and finalize epoch
    ctx.advance_epochs(1).await?;
    let finalize_event = ctx.finalize_epoch().await?;

    // Build the input using the wrong chain spec, Sepolia when Anvil is expected.
    let mint_input = ctx.build_mint_input_for_epochs_with_chain_spec(&[finalize_event.epoch.to()], &ETH_SEPOLIA_CHAIN_SPEC).await?;

    // Execute the mint calculator guest
    let mint_journal = common::execute_mint_calculator_guest(&mint_input)?;

    // Assemble a fake receipt and use it to call the mint function on the PovwMint contract.
    let mint_receipt: Receipt = FakeReceipt::new(ReceiptClaim::ok(
        BOUNDLESS_POVW_MINT_CALCULATOR_ID,
        mint_journal.abi_encode(),
    ))
    .try_into()?;

    // This should fail as chain spec is wrong.
    let result = ctx
        .mint_contract
        .mint(mint_journal.abi_encode().into(), common::encode_seal(&mint_receipt)?.into())
        .send()
        .await;

    assert!(result.is_err(), "Should reject wrong chain spec");
    let err = result.unwrap_err();
    // Check for InvalidSteelCommitment error selector 0xa7e6de3e
    assert!(err.to_string().contains("0xa7e6de3e"));

    Ok(())
}

#[tokio::test]
async fn mint_to_value_recipient() -> anyhow::Result<()> {
    let ctx = common::text_ctx().await?;
    let work_log_signer = PrivateKeySigner::random();
    let value_recipient = PrivateKeySigner::random();

    let initial_epoch = ctx.povw_contract.currentEpoch().call().await?;
    println!("Initial epoch: {initial_epoch}");

    // Work log controlled by work_log_signer, but rewards should go to value_recipient
    let update = LogBuilderJournal {
        self_image_id: RISC0_POVW_LOG_BUILDER_ID.into(),
        initial_commit: WorkLog::EMPTY.commit(),
        updated_commit: Digest::new(rand::random()),
        update_value: 50,
        work_log_id: work_log_signer.address().into(),
    };

    let work_log_event =
        ctx.post_work_log_update(&work_log_signer, &update, value_recipient.address()).await?;
    println!("Work log update posted for epoch {}", work_log_event.epochNumber);

    // Verify event has correct value recipient
    assert_eq!(work_log_event.workLogId, work_log_signer.address());
    assert_eq!(work_log_event.valueRecipient, value_recipient.address());

    // Advance time and finalize epoch
    ctx.advance_epochs(1).await?;
    let finalized_event = ctx.finalize_epoch().await?;

    assert_eq!(finalized_event.epoch, U256::from(initial_epoch));
    assert_eq!(finalized_event.totalWork, U256::from(50));

    // Run mint calculation
    let mint_receipt = ctx.run_mint().await?;
    println!("Mint transaction succeeded with {} gas used", mint_receipt.gas_used);

    // Check balances - value_recipient should get tokens, not work_log_signer
    let work_log_signer_balance =
        ctx.token_contract.balanceOf(work_log_signer.address()).call().await?;
    let value_recipient_balance =
        ctx.token_contract.balanceOf(value_recipient.address()).call().await?;
    let epoch_reward = ctx.mint_contract.EPOCH_REWARD().call().await?;

    assert_eq!(
        work_log_signer_balance,
        U256::ZERO,
        "Work log signer should not receive any tokens"
    );
    assert_eq!(
        value_recipient_balance, epoch_reward,
        "Value recipient should receive full epoch reward"
    );

    println!(
        "Verified: work_log_signer balance = {work_log_signer_balance}, value_recipient balance = {value_recipient_balance}"
    );

    Ok(())
}

#[tokio::test]
async fn single_work_log_multiple_recipients() -> anyhow::Result<()> {
    let ctx = common::text_ctx().await?;
    let work_log_signer = PrivateKeySigner::random();
    let recipient1 = PrivateKeySigner::random();
    let recipient2 = PrivateKeySigner::random();

    let initial_epoch = ctx.povw_contract.currentEpoch().call().await?;
    println!("Initial epoch: {initial_epoch}");

    // First update: work_log_signer -> recipient1
    let first_update = LogBuilderJournal {
        self_image_id: RISC0_POVW_LOG_BUILDER_ID.into(),
        initial_commit: WorkLog::EMPTY.commit(),
        updated_commit: Digest::new(rand::random()),
        update_value: 30,
        work_log_id: work_log_signer.address().into(),
    };

    let first_event =
        ctx.post_work_log_update(&work_log_signer, &first_update, recipient1.address()).await?;
    println!("First update: {} work units to recipient1", first_event.updateValue);

    // Second update: same work log, chained update -> recipient2
    let second_update = LogBuilderJournal {
        self_image_id: RISC0_POVW_LOG_BUILDER_ID.into(),
        initial_commit: first_update.updated_commit,
        updated_commit: Digest::new(rand::random()),
        update_value: 20,
        work_log_id: work_log_signer.address().into(),
    };

    let second_event =
        ctx.post_work_log_update(&work_log_signer, &second_update, recipient2.address()).await?;
    println!("Second update: {} work units to recipient2", second_event.updateValue);

    // Advance time and finalize epoch
    ctx.advance_epochs(1).await?;
    let finalized_event = ctx.finalize_epoch().await?;
    assert_eq!(finalized_event.totalWork, U256::from(50)); // 30 + 20

    // Run the full mint process
    let mint_receipt = ctx.run_mint().await?;
    println!("Mint transaction succeeded with {} gas used", mint_receipt.gas_used);

    // Check final token balances - should be proportional to work done
    let recipient1_balance = ctx.token_contract.balanceOf(recipient1.address()).call().await?;
    let recipient2_balance = ctx.token_contract.balanceOf(recipient2.address()).call().await?;
    let work_log_signer_balance =
        ctx.token_contract.balanceOf(work_log_signer.address()).call().await?;
    let epoch_reward = ctx.mint_contract.EPOCH_REWARD().call().await?;

    // Expected: recipient1 gets 30/50 = 60%, recipient2 gets 20/50 = 40%
    let expected_recipient1 = epoch_reward * U256::from(30) / U256::from(50);
    let expected_recipient2 = epoch_reward * U256::from(20) / U256::from(50);

    // Allow for small rounding errors in fixed-point arithmetic (within 10 wei)
    let tolerance = U256::from(10);

    assert_eq!(work_log_signer_balance, U256::ZERO, "Work log signer should not receive tokens");
    assert!(
        recipient1_balance.abs_diff(expected_recipient1) <= tolerance,
        "Recipient1 should get ~60% of epoch reward, got {recipient1_balance}, expected {expected_recipient1}"
    );
    assert!(
        recipient2_balance.abs_diff(expected_recipient2) <= tolerance,
        "Recipient2 should get ~40% of epoch reward, got {recipient2_balance}, expected {expected_recipient2}"
    );

    println!("Verified balances: recipient1={recipient1_balance}, recipient2={recipient2_balance}");

    Ok(())
}

#[tokio::test]
async fn multiple_work_logs_same_recipient() -> anyhow::Result<()> {
    let ctx = common::text_ctx().await?;
    let work_log_signer1 = PrivateKeySigner::random();
    let work_log_signer2 = PrivateKeySigner::random();
    let shared_recipient = PrivateKeySigner::random();

    let initial_epoch = ctx.povw_contract.currentEpoch().call().await?;
    println!("Initial epoch: {initial_epoch}");

    // First work log update -> shared_recipient
    let first_update = LogBuilderJournal {
        self_image_id: RISC0_POVW_LOG_BUILDER_ID.into(),
        initial_commit: WorkLog::EMPTY.commit(),
        updated_commit: Digest::new(rand::random()),
        update_value: 25,
        work_log_id: work_log_signer1.address().into(),
    };

    let first_event = ctx
        .post_work_log_update(&work_log_signer1, &first_update, shared_recipient.address())
        .await?;
    println!("First work log: {} work units to shared recipient", first_event.updateValue);

    // Second work log update -> same shared_recipient
    let second_update = LogBuilderJournal {
        self_image_id: RISC0_POVW_LOG_BUILDER_ID.into(),
        initial_commit: WorkLog::EMPTY.commit(),
        updated_commit: Digest::new(rand::random()),
        update_value: 35,
        work_log_id: work_log_signer2.address().into(),
    };

    let second_event = ctx
        .post_work_log_update(&work_log_signer2, &second_update, shared_recipient.address())
        .await?;
    println!("Second work log: {} work units to shared recipient", second_event.updateValue);

    // Advance time and finalize epoch
    ctx.advance_epochs(1).await?;
    let finalized_event = ctx.finalize_epoch().await?;
    assert_eq!(finalized_event.totalWork, U256::from(60)); // 25 + 35

    // Run the full mint process
    let mint_receipt = ctx.run_mint().await?;
    println!("Mint transaction succeeded with {} gas used", mint_receipt.gas_used);

    // Check final token balances
    let shared_recipient_balance =
        ctx.token_contract.balanceOf(shared_recipient.address()).call().await?;
    let work_log_signer1_balance =
        ctx.token_contract.balanceOf(work_log_signer1.address()).call().await?;
    let work_log_signer2_balance =
        ctx.token_contract.balanceOf(work_log_signer2.address()).call().await?;
    let epoch_reward = ctx.mint_contract.EPOCH_REWARD().call().await?;

    // Shared recipient should get the full epoch reward (100% since they get all the work from both logs)
    // Allow for small rounding errors in fixed-point arithmetic (within 10 wei)
    let tolerance = U256::from(10);

    assert_eq!(work_log_signer1_balance, U256::ZERO, "Work log signer1 should not receive tokens");
    assert_eq!(work_log_signer2_balance, U256::ZERO, "Work log signer2 should not receive tokens");
    assert!(
        shared_recipient_balance.abs_diff(epoch_reward) <= tolerance,
        "Shared recipient should get ~full epoch reward, got {shared_recipient_balance}, expected {epoch_reward}"
    );

    println!("Verified: shared_recipient balance = {shared_recipient_balance}");

    Ok(())
}

#[tokio::test]
async fn zero_valued_update() -> anyhow::Result<()> {
    let ctx = common::text_ctx().await?;
    let signer = PrivateKeySigner::random();

    let initial_epoch = ctx.povw_contract.currentEpoch().call().await?;
    println!("Initial epoch: {initial_epoch}");

    // Post a zero-valued work log update
    let update = LogBuilderJournal {
        self_image_id: RISC0_POVW_LOG_BUILDER_ID.into(),
        initial_commit: WorkLog::EMPTY.commit(),
        updated_commit: Digest::new(rand::random()),
        update_value: 0, // Zero-valued update
        work_log_id: signer.address().into(),
    };

    let work_log_event = ctx.post_work_log_update(&signer, &update, signer.address()).await?;
    println!("Zero-valued work log update posted for epoch {}", work_log_event.epochNumber);

    // Verify the update was accepted with zero value
    assert_eq!(work_log_event.updateValue, U256::ZERO);
    assert_eq!(work_log_event.updatedCommit, B256::from(<[u8; 32]>::from(update.updated_commit)));

    // Advance time and finalize epoch
    ctx.advance_epochs(1).await?;
    let finalized_event = ctx.finalize_epoch().await?;

    // The epoch should be finalized with zero total work
    assert_eq!(finalized_event.epoch, U256::from(initial_epoch));
    assert_eq!(finalized_event.totalWork, U256::ZERO);
    println!("EpochFinalized event verified: epoch={}, totalWork=0", finalized_event.epoch);

    // Run the mint process - should complete successfully
    ctx.run_mint_for_epochs(&[finalized_event.epoch.to()]).await?;

    // Verify no tokens were minted (recipient balance should remain zero)
    let zero_update_balance = ctx.token_contract.balanceOf(signer.address()).call().await?;
    assert_eq!(zero_update_balance, U256::ZERO, "No tokens should be minted for zero-valued updates");

    // Run a second update starting from the previous one, to ensure that although no tokens were
    // minted, the work log commit was updated.
    let second_update = LogBuilderJournal {
        self_image_id: RISC0_POVW_LOG_BUILDER_ID.into(),
        initial_commit: update.updated_commit,
        updated_commit: Digest::new(rand::random()),
        update_value: 100, // Zero-valued update
        work_log_id: signer.address().into(),
    };

    ctx.post_work_log_update(&signer, &second_update, signer.address()).await?;
    ctx.advance_epochs(1).await?;
    let finalized_event = ctx.finalize_epoch().await?;

    ctx.run_mint_for_epochs(&[finalized_event.epoch.to()]).await?;

    // Verify tokens were minted this time.
    let final_balance = ctx.token_contract.balanceOf(signer.address()).call().await?;
    assert_eq!(final_balance, ctx.mint_contract.EPOCH_REWARD().call().await?);
    Ok(())
}
