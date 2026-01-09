// Copyright 2026 Boundless Foundation, Inc.
//
// Use of this source code is governed by the Business Source License
// as found in the LICENSE-BSL file.

use alloy::{
    primitives::{B256, U256},
    providers::{ext::AnvilApi, Provider},
    signers::local::PrivateKeySigner,
};
use alloy_sol_types::SolValue;

use boundless_povw::{
    log_updater::LogBuilderJournal,
    mint_calculator::{
        MintCalculatorJournal, MintCalculatorMint, MintCalculatorUpdate, WorkLogFilter,
        BOUNDLESS_POVW_MINT_CALCULATOR_ID,
    },
};
use boundless_test_utils::povw::{
    execute_mint_calculator_guest, test_ctx, test_ctx_with, MintOptions,
};
use risc0_ethereum_contracts::encode_seal;
use risc0_povw::guest::RISC0_POVW_LOG_BUILDER_ID;
use risc0_povw::WorkLog;
use risc0_steel::ethereum::ETH_SEPOLIA_CHAIN_SPEC;
use risc0_zkvm::{Digest, FakeReceipt, Receipt, ReceiptClaim};

#[test]
fn use_blst() {
    // call something some blst to mitigate a build issue where is does not become linked.
    let _ = unsafe { blst::blst_p1_sizeof() };
}

#[tokio::test]
async fn basic() -> anyhow::Result<()> {
    // Setup test context
    let ctx = test_ctx().await?;

    let initial_epoch = ctx.zkc.getCurrentEpoch().call().await?;
    println!("Initial epoch: {initial_epoch}");

    // Post a work log update
    let signer = PrivateKeySigner::random();
    let update = LogBuilderJournal::builder()
        .self_image_id(RISC0_POVW_LOG_BUILDER_ID)
        .initial_commit(WorkLog::EMPTY.commit())
        .updated_commit(Digest::new(rand::random()))
        .update_value(25) // Work value for this update
        .work_log_id(signer.address())
        .build()
        .unwrap();

    let work_log_event = ctx.post_work_log_update(&signer, &update, signer.address()).await?;
    println!("Work log update posted for epoch {}", work_log_event.epochNumber);

    // Advance time and finalize epoch
    ctx.advance_epochs(U256::ONE).await?;
    let finalized_event = ctx.finalize_epoch().await?;

    assert_eq!(finalized_event.epoch, U256::from(initial_epoch));
    assert_eq!(finalized_event.totalWork, U256::from(25)); // Our work log update value
    println!(
        "EpochFinalized event verified: epoch={}, totalWork={}",
        finalized_event.epoch, finalized_event.totalWork
    );

    let mint_receipt = ctx.run_mint().await?;
    println!("Mint transaction succeeded with {} gas used", mint_receipt.gas_used);

    let final_balance = ctx.zkc.balanceOf(signer.address()).call().await?;
    let epoch_reward = ctx.zkc.getPoVWEmissionsForEpoch(finalized_event.epoch).call().await?;

    assert_eq!(final_balance, epoch_reward, "Minted amount should match expected calculation");
    Ok(())
}

#[tokio::test]
async fn proportional_rewards_same_epoch() -> anyhow::Result<()> {
    let ctx = test_ctx().await?;

    let initial_epoch = ctx.zkc.getCurrentEpoch().call().await?;
    println!("Initial epoch: {initial_epoch}");

    let signer1 = PrivateKeySigner::random();
    let signer2 = PrivateKeySigner::random();

    // First update: 30 work units
    let update1 = LogBuilderJournal::builder()
        .self_image_id(RISC0_POVW_LOG_BUILDER_ID)
        .initial_commit(WorkLog::EMPTY.commit())
        .updated_commit(Digest::new(rand::random()))
        .update_value(30)
        .work_log_id(signer1.address())
        .build()
        .unwrap();

    // Second update: 70 work units (different log ID)
    let update2 = LogBuilderJournal::builder()
        .self_image_id(RISC0_POVW_LOG_BUILDER_ID)
        .initial_commit(WorkLog::EMPTY.commit())
        .updated_commit(Digest::new(rand::random()))
        .update_value(70)
        .work_log_id(signer2.address())
        .build()
        .unwrap();

    let event1 = ctx.post_work_log_update(&signer1, &update1, signer1.address()).await?;
    let event2 = ctx.post_work_log_update(&signer2, &update2, signer2.address()).await?;

    println!("Update 1: {} work units for {:?}", event1.updateValue, event1.workLogId);
    println!("Update 2: {} work units for {:?}", event2.updateValue, event2.workLogId);

    // Advance time and finalize epoch
    ctx.advance_epochs(U256::ONE).await?;
    let finalized_event = ctx.finalize_epoch().await?;

    // Total work should be 30 + 70 = 100
    assert_eq!(finalized_event.totalWork, U256::from(100));
    println!("Total work in epoch: {}", finalized_event.totalWork);

    // Run mint calculation
    let mint_receipt = ctx.run_mint().await?;
    println!("Mint transaction succeeded with {} gas used", mint_receipt.gas_used);

    // Check balances - should be proportional to work done
    let balance1 = ctx.zkc.balanceOf(signer1.address()).call().await?;
    let balance2 = ctx.zkc.balanceOf(signer2.address()).call().await?;
    let epoch_reward = ctx.zkc.getPoVWEmissionsForEpoch(finalized_event.epoch).call().await?;

    // Expected: signer1 gets 30%, signer2 gets 70%
    let expected1 = epoch_reward * U256::from(30) / U256::from(100);
    let expected2 = epoch_reward * U256::from(70) / U256::from(100);

    // Allow for small rounding errors in fixed-point arithmetic (within 10 wei)
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
    let ctx = test_ctx().await?;

    let first_epoch = ctx.zkc.getCurrentEpoch().call().await?;
    println!("Starting epoch: {first_epoch}");

    let signer = PrivateKeySigner::random();

    // First epoch update
    let update1 = LogBuilderJournal::builder()
        .self_image_id(RISC0_POVW_LOG_BUILDER_ID)
        .initial_commit(WorkLog::EMPTY.commit())
        .updated_commit(Digest::new(rand::random()))
        .update_value(50)
        .work_log_id(signer.address())
        .build()
        .unwrap();

    let event1 = ctx.post_work_log_update(&signer, &update1, signer.address()).await?;
    println!("Update 1: {} work units in epoch {}", event1.updateValue, event1.epochNumber);

    // Advance to next epoch and finalize first epoch
    ctx.advance_epochs(U256::ONE).await?;
    let finalized_event1 = ctx.finalize_epoch().await?;
    assert_eq!(finalized_event1.totalWork, U256::from(50));

    // First mint for first epoch
    let mint_receipt1 =
        ctx.run_mint_with_opts(MintOptions::builder().epochs([first_epoch])).await?;
    println!("First mint completed with {} gas used", mint_receipt1.gas_used);

    let balance_after_first_mint = ctx.zkc.balanceOf(signer.address()).call().await?;
    let epoch_reward = ctx.zkc.getPoVWEmissionsForEpoch(finalized_event1.epoch).call().await?;

    assert_eq!(
        balance_after_first_mint, epoch_reward,
        "After first mint should have full epoch reward"
    );
    println!("Balance after first mint: {balance_after_first_mint} tokens");

    // Second epoch update (chained from first)
    let second_epoch = ctx.zkc.getCurrentEpoch().call().await?;
    let update2 = LogBuilderJournal::builder()
        .self_image_id(RISC0_POVW_LOG_BUILDER_ID)
        .initial_commit(update1.updated_commit) // Chain from first update
        .updated_commit(Digest::new(rand::random()))
        .update_value(75)
        .work_log_id(signer.address())
        .build()
        .unwrap();

    let event2 = ctx.post_work_log_update(&signer, &update2, signer.address()).await?;
    println!("Update 2: {} work units in epoch {}", event2.updateValue, event2.epochNumber);

    // Advance to next epoch and finalize second epoch
    ctx.advance_epochs(U256::ONE).await?;
    let finalized_event2 = ctx.finalize_epoch().await?;
    assert_eq!(finalized_event2.epoch, U256::from(second_epoch));
    assert_eq!(finalized_event2.totalWork, U256::from(75));

    // Second mint for second epoch
    let mint_receipt2 =
        ctx.run_mint_with_opts(MintOptions::builder().epochs([second_epoch])).await?;
    println!("Second mint completed with {} gas used", mint_receipt2.gas_used);

    let final_balance = ctx.zkc.balanceOf(signer.address()).call().await?;
    let expected_total = epoch_reward * U256::from(2); // Both full epoch rewards

    assert_eq!(final_balance, expected_total, "Final balance should be exactly 2x epoch reward");
    println!("Final balance after both mints: {final_balance} tokens");

    Ok(())
}

#[tokio::test]
async fn cross_epoch_mint() -> anyhow::Result<()> {
    let ctx = test_ctx().await?;

    let first_epoch = ctx.zkc.getCurrentEpoch().call().await?;
    println!("Starting epoch: {first_epoch}");

    let signer = PrivateKeySigner::random();

    // First epoch update
    let update1 = LogBuilderJournal::builder()
        .self_image_id(RISC0_POVW_LOG_BUILDER_ID)
        .initial_commit(WorkLog::EMPTY.commit())
        .updated_commit(Digest::new(rand::random()))
        .update_value(40)
        .work_log_id(signer.address())
        .build()
        .unwrap();

    let event1 = ctx.post_work_log_update(&signer, &update1, signer.address()).await?;
    println!("Update 1: {} work units in epoch {}", event1.updateValue, event1.epochNumber);

    // Advance to next epoch and finalize first epoch
    ctx.advance_epochs(U256::ONE).await?;
    let finalized_event1 = ctx.finalize_epoch().await?;
    assert_eq!(finalized_event1.totalWork, U256::from(40));

    // Second epoch update (chained from first)
    let second_epoch = ctx.zkc.getCurrentEpoch().call().await?;
    let update2 = LogBuilderJournal::builder()
        .self_image_id(RISC0_POVW_LOG_BUILDER_ID)
        .initial_commit(update1.updated_commit) // Chain from first update
        .updated_commit(Digest::new(rand::random()))
        .update_value(60)
        .work_log_id(signer.address())
        .build()
        .unwrap();

    let event2 = ctx.post_work_log_update(&signer, &update2, signer.address()).await?;
    println!("Update 2: {} work units in epoch {}", event2.updateValue, event2.epochNumber);

    // Advance to next epoch and finalize second epoch
    ctx.advance_epochs(U256::ONE).await?;
    let finalized_event2 = ctx.finalize_epoch().await?;
    assert_eq!(finalized_event2.epoch, U256::from(second_epoch));
    assert_eq!(finalized_event2.totalWork, U256::from(60));

    // Single mint covering both epochs
    let mint_receipt =
        ctx.run_mint_with_opts(MintOptions::builder().epochs([first_epoch, second_epoch])).await?;
    println!("Cross-epoch mint completed with {} gas used", mint_receipt.gas_used);

    let final_balance = ctx.zkc.balanceOf(signer.address()).call().await?;
    let epoch_reward = ctx.zkc.getPoVWEmissionsForEpoch(finalized_event2.epoch).call().await?;
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
    let ctx = test_ctx().await?;
    let signer = PrivateKeySigner::random();

    // Setup a basic work log and epoch
    let update = LogBuilderJournal::builder()
        .self_image_id(RISC0_POVW_LOG_BUILDER_ID)
        .initial_commit(WorkLog::EMPTY.commit())
        .updated_commit(Digest::new(rand::random()))
        .update_value(25)
        .work_log_id(signer.address())
        .build()
        .unwrap();

    let _work_log_event = ctx.post_work_log_update(&signer, &update, signer.address()).await?;
    ctx.advance_epochs(U256::ONE).await?;
    let _finalized_event = ctx.finalize_epoch().await?;

    // Create a mint journal with invalid Steel commitment
    let mint_journal = MintCalculatorJournal {
        mints: vec![MintCalculatorMint { recipient: signer.address(), value: U256::ONE }],
        updates: vec![MintCalculatorUpdate {
            workLogId: signer.address(),
            initialCommit: B256::from(<[u8; 32]>::from(update.initial_commit)),
            updatedCommit: B256::from(<[u8; 32]>::from(update.updated_commit)),
        }],
        povwAccountingAddress: *ctx.povw_accounting.address(),
        zkcAddress: *ctx.zkc.address(),
        zkcRewardsAddress: *ctx.zkc_rewards.address(),
        steelCommit: risc0_steel::Commitment::default(), // Invalid/empty Steel commitment
    };

    // Create fake receipt and try to submit
    let fake_receipt = FakeReceipt::new(ReceiptClaim::ok(
        BOUNDLESS_POVW_MINT_CALCULATOR_ID,
        mint_journal.abi_encode(),
    ));
    let receipt: Receipt = fake_receipt.try_into()?;

    let result = ctx
        .povw_mint
        .mint(mint_journal.abi_encode().into(), encode_seal(&receipt)?.into())
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
    let ctx1 = test_ctx().await?;
    let ctx2 = test_ctx_with(ctx1.anvil.clone(), 1).await?;

    let signer = PrivateKeySigner::random();
    let update = LogBuilderJournal::builder()
        .self_image_id(RISC0_POVW_LOG_BUILDER_ID)
        .initial_commit(WorkLog::EMPTY.commit())
        .updated_commit(Digest::new(rand::random()))
        .update_value(25) // Work value for this update
        .work_log_id(signer.address())
        .build()
        .unwrap();

    // Using deployment #1, build the mint inputs.
    ctx1.post_work_log_update(&signer, &update, signer.address()).await?;
    ctx1.advance_epochs(U256::ONE).await?;
    ctx1.finalize_epoch().await?;

    let mint_input = ctx1.build_mint_input(MintOptions::default()).await?;

    // Execute the mint calculator guest
    let mint_journal = execute_mint_calculator_guest(&mint_input)?;

    // Assemble a fake receipt and use it to call the mint function on the PovwMint contract.
    let mint_receipt: Receipt = FakeReceipt::new(ReceiptClaim::ok(
        BOUNDLESS_POVW_MINT_CALCULATOR_ID,
        mint_journal.abi_encode(),
    ))
    .try_into()?;

    // Submit the mint to deployment #2. This should fail as the contract address for the PovwAccounting
    // contract is wrong.
    let result = ctx2
        .povw_mint
        .mint(mint_journal.abi_encode().into(), encode_seal(&mint_receipt)?.into())
        .send()
        .await;

    assert!(result.is_err(), "Should reject wrong PovwAccounting contract address");
    let err = result.unwrap_err();
    println!("Contract correctly rejected wrong PovwAccounting address: {err}");
    // Check for IncorrectPovwAddress error selector 0x82db2de2
    assert!(err.to_string().contains("0x98d6328f"));

    Ok(())
}

#[tokio::test]
async fn reject_mint_with_only_latter_epoch() -> anyhow::Result<()> {
    let ctx = test_ctx().await?;
    let signer = PrivateKeySigner::random();

    let _first_epoch = ctx.zkc.getCurrentEpoch().call().await?;

    // First update in first epoch
    let update1 = LogBuilderJournal::builder()
        .self_image_id(RISC0_POVW_LOG_BUILDER_ID)
        .initial_commit(WorkLog::EMPTY.commit())
        .updated_commit(Digest::new(rand::random()))
        .update_value(30)
        .work_log_id(signer.address())
        .build()
        .unwrap();

    ctx.post_work_log_update(&signer, &update1, signer.address()).await?;
    ctx.advance_epochs(U256::ONE).await?;
    ctx.finalize_epoch().await?;

    let second_epoch = ctx.zkc.getCurrentEpoch().call().await?;

    // Second update in second epoch (chained from first)
    let update2 = LogBuilderJournal::builder()
        .self_image_id(RISC0_POVW_LOG_BUILDER_ID)
        .initial_commit(update1.updated_commit)
        .updated_commit(Digest::new(rand::random()))
        .update_value(40)
        .work_log_id(signer.address())
        .build()
        .unwrap();

    ctx.post_work_log_update(&signer, &update2, signer.address()).await?;
    ctx.advance_epochs(U256::ONE).await?;
    ctx.finalize_epoch().await?;

    // Try to mint using only the second epoch - should fail
    let result = ctx.run_mint_with_opts(MintOptions::builder().epochs([second_epoch])).await;
    assert!(result.is_err(), "Should reject mint with incomplete chain");
    let err = result.unwrap_err();
    println!("Contract correctly rejected incomplete chain: {err}");
    // Check for IncorrectInitialUpdateCommit error selector 0xf4a2b615
    assert!(err.to_string().contains("0xf4a2b615"));

    Ok(())
}

#[tokio::test]
async fn reject_mint_with_skipped_epoch() -> anyhow::Result<()> {
    let ctx = test_ctx().await?;
    let signer = PrivateKeySigner::random();

    let first_epoch = ctx.zkc.getCurrentEpoch().call().await?;

    // First update in first epoch
    let update1 = LogBuilderJournal::builder()
        .self_image_id(RISC0_POVW_LOG_BUILDER_ID)
        .initial_commit(WorkLog::EMPTY.commit())
        .updated_commit(Digest::new(rand::random()))
        .update_value(20)
        .work_log_id(signer.address())
        .build()
        .unwrap();

    ctx.post_work_log_update(&signer, &update1, signer.address()).await?;
    ctx.advance_epochs(U256::ONE).await?;
    ctx.finalize_epoch().await?;

    let _second_epoch = ctx.zkc.getCurrentEpoch().call().await?;

    // Second update in second epoch (chained from first)
    let update2 = LogBuilderJournal::builder()
        .self_image_id(RISC0_POVW_LOG_BUILDER_ID)
        .initial_commit(update1.updated_commit)
        .updated_commit(Digest::new(rand::random()))
        .update_value(30)
        .work_log_id(signer.address())
        .build()
        .unwrap();

    ctx.post_work_log_update(&signer, &update2, signer.address()).await?;
    ctx.advance_epochs(U256::ONE).await?;
    ctx.finalize_epoch().await?;

    let third_epoch = ctx.zkc.getCurrentEpoch().call().await?;

    // Third update in third epoch (chained from second)
    let update3 = LogBuilderJournal::builder()
        .self_image_id(RISC0_POVW_LOG_BUILDER_ID)
        .initial_commit(update2.updated_commit)
        .updated_commit(Digest::new(rand::random()))
        .update_value(50)
        .work_log_id(signer.address())
        .build()
        .unwrap();

    ctx.post_work_log_update(&signer, &update3, signer.address()).await?;
    ctx.advance_epochs(U256::ONE).await?;
    ctx.finalize_epoch().await?;

    // Try to mint using first and third epochs (skipping second) - should fail
    let result =
        ctx.run_mint_with_opts(MintOptions::builder().epochs([first_epoch, third_epoch])).await;
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
async fn mint_with_one_finalized_and_one_unfinalized_epoch() -> anyhow::Result<()> {
    let ctx = test_ctx().await?;
    let signer = PrivateKeySigner::random();

    // First update in first epoch
    let update1 = LogBuilderJournal::builder()
        .self_image_id(RISC0_POVW_LOG_BUILDER_ID)
        .initial_commit(WorkLog::EMPTY.commit())
        .updated_commit(Digest::new(rand::random()))
        .update_value(20)
        .work_log_id(signer.address())
        .build()
        .unwrap();

    ctx.post_work_log_update(&signer, &update1, signer.address()).await?;
    ctx.advance_epochs(U256::ONE).await?;
    let finalize_event = ctx.finalize_epoch().await?;

    // Post work log update in the seconds epoch but don't finalize the epoch
    let update2 = LogBuilderJournal::builder()
        .self_image_id(RISC0_POVW_LOG_BUILDER_ID)
        .initial_commit(update1.updated_commit)
        .updated_commit(Digest::new(rand::random()))
        .update_value(25)
        .work_log_id(signer.address())
        .build()
        .unwrap();

    ctx.post_work_log_update(&signer, &update2, signer.address()).await?;

    // Advance time but DO NOT finalize the epoch
    ctx.advance_epochs(U256::ONE).await?;

    // Single mint covering both epochs
    ctx.run_mint_with_opts(MintOptions::builder()).await?;

    let final_balance = ctx.zkc.balanceOf(signer.address()).call().await?;
    let epoch_reward = ctx.zkc.getPoVWEmissionsForEpoch(finalize_event.epoch).call().await?;
    let expected_total = epoch_reward * U256::from(1); // Just the reward for the first epoch.

    assert_eq!(
        final_balance, expected_total,
        "Final balance should be exactly epoch reward from the finalized epoch"
    );
    println!("Final balance after cross-epoch mint: {final_balance} tokens");

    Ok(())
}

#[tokio::test]
async fn reject_mint_with_unfinalized_epoch() -> anyhow::Result<()> {
    let ctx = test_ctx().await?;
    let signer = PrivateKeySigner::random();

    let current_epoch = ctx.zkc.getCurrentEpoch().call().await?;

    // Post work log update but don't finalize the epoch
    let update = LogBuilderJournal::builder()
        .self_image_id(RISC0_POVW_LOG_BUILDER_ID)
        .initial_commit(WorkLog::EMPTY.commit())
        .updated_commit(Digest::new(rand::random()))
        .update_value(25)
        .work_log_id(signer.address())
        .build()
        .unwrap();

    ctx.post_work_log_update(&signer, &update, signer.address()).await?;

    // Advance time but DO NOT finalize the epoch
    ctx.advance_epochs(U256::ONE).await?;

    // Try to mint without finalizing the epoch - should fail
    let result = ctx.run_mint_with_opts(MintOptions::builder().epochs([current_epoch])).await;
    assert!(result.is_err(), "Should reject mint with unfinalized epoch");
    let err = result.unwrap_err();
    println!("Contract correctly rejected unfinalized epoch: {err}");
    // The mint calculator guest should fail because there's no EpochFinalized event
    //assert!(err.to_string().contains("no epoch finalized event processed"));
    // TODO(victor): This test currently fails before getting to the guest. Provide a way to advance
    // the preflight (skipping some steps) to build an input to at least let the guest run.
    assert!(err.to_string().contains("No EpochFinalized events in the given blocks"));

    Ok(())
}

#[tokio::test]
async fn reject_mint_wrong_chain_spec() -> anyhow::Result<()> {
    // Setup test context
    let ctx = test_ctx().await?;

    // Post a work log update
    let signer = PrivateKeySigner::random();
    let update = LogBuilderJournal::builder()
        .self_image_id(RISC0_POVW_LOG_BUILDER_ID)
        .initial_commit(WorkLog::EMPTY.commit())
        .updated_commit(Digest::new(rand::random()))
        .update_value(25) // Work value for this update
        .work_log_id(signer.address())
        .build()
        .unwrap();

    ctx.post_work_log_update(&signer, &update, signer.address()).await?;

    // Advance time and finalize epoch
    ctx.advance_epochs(U256::ONE).await?;
    let finalize_event = ctx.finalize_epoch().await?;

    // Build the input using the wrong chain spec, Sepolia when Anvil is expected.
    let mint_input = ctx
        .build_mint_input(
            MintOptions::builder()
                .epochs([finalize_event.epoch])
                .chain_spec(&ETH_SEPOLIA_CHAIN_SPEC),
        )
        .await?;

    // Execute the mint calculator guest
    let mint_journal = execute_mint_calculator_guest(&mint_input)?;

    // Assemble a fake receipt and use it to call the mint function on the PovwMint contract.
    let mint_receipt: Receipt = FakeReceipt::new(ReceiptClaim::ok(
        BOUNDLESS_POVW_MINT_CALCULATOR_ID,
        mint_journal.abi_encode(),
    ))
    .try_into()?;

    // This should fail as chain spec is wrong.
    let result = ctx
        .povw_mint
        .mint(mint_journal.abi_encode().into(), encode_seal(&mint_receipt)?.into())
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
    let ctx = test_ctx().await?;
    let work_log_signer = PrivateKeySigner::random();
    let value_recipient = PrivateKeySigner::random();

    let initial_epoch = ctx.zkc.getCurrentEpoch().call().await?;
    println!("Initial epoch: {initial_epoch}");

    // Work log controlled by work_log_signer, but rewards should go to value_recipient
    let update = LogBuilderJournal::builder()
        .self_image_id(RISC0_POVW_LOG_BUILDER_ID)
        .initial_commit(WorkLog::EMPTY.commit())
        .updated_commit(Digest::new(rand::random()))
        .update_value(50)
        .work_log_id(work_log_signer.address())
        .build()
        .unwrap();

    let work_log_event =
        ctx.post_work_log_update(&work_log_signer, &update, value_recipient.address()).await?;
    println!("Work log update posted for epoch {}", work_log_event.epochNumber);

    // Verify event has correct value recipient
    assert_eq!(work_log_event.workLogId, work_log_signer.address());
    assert_eq!(work_log_event.valueRecipient, value_recipient.address());

    // Advance time and finalize epoch
    ctx.advance_epochs(U256::ONE).await?;
    let finalized_event = ctx.finalize_epoch().await?;

    assert_eq!(finalized_event.epoch, U256::from(initial_epoch));
    assert_eq!(finalized_event.totalWork, U256::from(50));

    // Run mint calculation
    let mint_receipt = ctx.run_mint().await?;
    println!("Mint transaction succeeded with {} gas used", mint_receipt.gas_used);

    // Check balances - value_recipient should get tokens, not work_log_signer
    let work_log_signer_balance = ctx.zkc.balanceOf(work_log_signer.address()).call().await?;
    let value_recipient_balance = ctx.zkc.balanceOf(value_recipient.address()).call().await?;
    let epoch_reward = ctx.zkc.getPoVWEmissionsForEpoch(finalized_event.epoch).call().await?;

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
    let ctx = test_ctx().await?;
    let work_log_signer = PrivateKeySigner::random();
    let recipient1 = PrivateKeySigner::random();
    let recipient2 = PrivateKeySigner::random();

    let initial_epoch = ctx.zkc.getCurrentEpoch().call().await?;
    println!("Initial epoch: {initial_epoch}");

    // First update: work_log_signer -> recipient1
    let first_update = LogBuilderJournal::builder()
        .self_image_id(RISC0_POVW_LOG_BUILDER_ID)
        .initial_commit(WorkLog::EMPTY.commit())
        .updated_commit(Digest::new(rand::random()))
        .update_value(30)
        .work_log_id(work_log_signer.address())
        .build()
        .unwrap();

    let first_event =
        ctx.post_work_log_update(&work_log_signer, &first_update, recipient1.address()).await?;
    println!("First update: {} work units to recipient1", first_event.updateValue);

    // Second update: same work log, chained update -> recipient2
    let second_update = LogBuilderJournal::builder()
        .self_image_id(RISC0_POVW_LOG_BUILDER_ID)
        .initial_commit(first_update.updated_commit)
        .updated_commit(Digest::new(rand::random()))
        .update_value(20)
        .work_log_id(work_log_signer.address())
        .build()
        .unwrap();

    let second_event =
        ctx.post_work_log_update(&work_log_signer, &second_update, recipient2.address()).await?;
    println!("Second update: {} work units to recipient2", second_event.updateValue);

    // Advance time and finalize epoch
    ctx.advance_epochs(U256::ONE).await?;
    let finalized_event = ctx.finalize_epoch().await?;
    assert_eq!(finalized_event.totalWork, U256::from(50)); // 30 + 20

    // Run the full mint process
    let mint_receipt = ctx.run_mint().await?;
    println!("Mint transaction succeeded with {} gas used", mint_receipt.gas_used);

    // Check final token balances - should be proportional to work done
    let recipient1_balance = ctx.zkc.balanceOf(recipient1.address()).call().await?;
    let recipient2_balance = ctx.zkc.balanceOf(recipient2.address()).call().await?;
    let work_log_signer_balance = ctx.zkc.balanceOf(work_log_signer.address()).call().await?;
    let epoch_reward = ctx.zkc.getPoVWEmissionsForEpoch(finalized_event.epoch).call().await?;

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
    let ctx = test_ctx().await?;
    let work_log_signer1 = PrivateKeySigner::random();
    let work_log_signer2 = PrivateKeySigner::random();
    let shared_recipient = PrivateKeySigner::random();

    let initial_epoch = ctx.zkc.getCurrentEpoch().call().await?;
    println!("Initial epoch: {initial_epoch}");

    // First work log update -> shared_recipient
    let first_update = LogBuilderJournal::builder()
        .self_image_id(RISC0_POVW_LOG_BUILDER_ID)
        .initial_commit(WorkLog::EMPTY.commit())
        .updated_commit(Digest::new(rand::random()))
        .update_value(25)
        .work_log_id(work_log_signer1.address())
        .build()
        .unwrap();

    let first_event = ctx
        .post_work_log_update(&work_log_signer1, &first_update, shared_recipient.address())
        .await?;
    println!("First work log: {} work units to shared recipient", first_event.updateValue);

    // Second work log update -> same shared_recipient
    let second_update = LogBuilderJournal::builder()
        .self_image_id(RISC0_POVW_LOG_BUILDER_ID)
        .initial_commit(WorkLog::EMPTY.commit())
        .updated_commit(Digest::new(rand::random()))
        .update_value(35)
        .work_log_id(work_log_signer2.address())
        .build()
        .unwrap();

    let second_event = ctx
        .post_work_log_update(&work_log_signer2, &second_update, shared_recipient.address())
        .await?;
    println!("Second work log: {} work units to shared recipient", second_event.updateValue);

    // Advance time and finalize epoch
    ctx.advance_epochs(U256::ONE).await?;
    let finalized_event = ctx.finalize_epoch().await?;
    assert_eq!(finalized_event.totalWork, U256::from(60)); // 25 + 35

    // Run the full mint process
    let mint_receipt = ctx.run_mint().await?;
    println!("Mint transaction succeeded with {} gas used", mint_receipt.gas_used);

    // Check final token balances
    let shared_recipient_balance = ctx.zkc.balanceOf(shared_recipient.address()).call().await?;
    let work_log_signer1_balance = ctx.zkc.balanceOf(work_log_signer1.address()).call().await?;
    let work_log_signer2_balance = ctx.zkc.balanceOf(work_log_signer2.address()).call().await?;
    let epoch_reward = ctx.zkc.getPoVWEmissionsForEpoch(finalized_event.epoch).call().await?;

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
    let ctx = test_ctx().await?;
    let signer = PrivateKeySigner::random();

    let initial_epoch = ctx.zkc.getCurrentEpoch().call().await?;
    println!("Initial epoch: {initial_epoch}");

    // Post a zero-valued work log update
    let update = LogBuilderJournal::builder()
        .self_image_id(RISC0_POVW_LOG_BUILDER_ID)
        .initial_commit(WorkLog::EMPTY.commit())
        .updated_commit(Digest::new(rand::random()))
        .update_value(0) // Zero-valued update
        .work_log_id(signer.address())
        .build()
        .unwrap();

    let work_log_event = ctx.post_work_log_update(&signer, &update, signer.address()).await?;
    println!("Zero-valued work log update posted for epoch {}", work_log_event.epochNumber);

    // Verify the update was accepted with zero value
    assert_eq!(work_log_event.updateValue, U256::ZERO);
    assert_eq!(work_log_event.updatedCommit, B256::from(<[u8; 32]>::from(update.updated_commit)));

    // Advance time and finalize epoch
    ctx.advance_epochs(U256::ONE).await?;
    let finalized_event = ctx.finalize_epoch().await?;

    // The epoch should be finalized with zero total work
    assert_eq!(finalized_event.epoch, U256::from(initial_epoch));
    assert_eq!(finalized_event.totalWork, U256::ZERO);
    println!("EpochFinalized event verified: epoch={}, totalWork=0", finalized_event.epoch);

    // Run the mint process - should complete successfully
    ctx.run_mint_with_opts(MintOptions::builder().epochs([finalized_event.epoch])).await?;

    // Verify no tokens were minted (recipient balance should remain zero)
    let zero_update_balance = ctx.zkc.balanceOf(signer.address()).call().await?;
    assert_eq!(
        zero_update_balance,
        U256::ZERO,
        "No tokens should be minted for zero-valued updates"
    );

    // Run a second update starting from the previous one, to ensure that although no tokens were
    // minted, the work log commit was updated.
    let second_update = LogBuilderJournal::builder()
        .self_image_id(RISC0_POVW_LOG_BUILDER_ID)
        .initial_commit(update.updated_commit)
        .updated_commit(Digest::new(rand::random()))
        .update_value(100) // Zero-valued update
        .work_log_id(signer.address())
        .build()
        .unwrap();

    ctx.post_work_log_update(&signer, &second_update, signer.address()).await?;
    ctx.advance_epochs(U256::ONE).await?;
    let finalized_event = ctx.finalize_epoch().await?;

    ctx.run_mint_with_opts(MintOptions::builder().epochs([finalized_event.epoch])).await?;

    // Verify tokens were minted this time.
    let final_balance = ctx.zkc.balanceOf(signer.address()).call().await?;
    assert_eq!(
        final_balance,
        ctx.zkc.getPoVWEmissionsForEpoch(finalized_event.epoch).call().await?
    );
    Ok(())
}

#[tokio::test]
async fn filter_individual_work_log_mints() -> anyhow::Result<()> {
    let ctx = test_ctx().await?;

    let initial_epoch = ctx.zkc.getCurrentEpoch().call().await?;
    println!("Initial epoch: {initial_epoch}");

    let signer_a = PrivateKeySigner::random();
    let signer_b = PrivateKeySigner::random();

    // Work log A: 30 work units
    let update_a = LogBuilderJournal::builder()
        .self_image_id(RISC0_POVW_LOG_BUILDER_ID)
        .initial_commit(WorkLog::EMPTY.commit())
        .updated_commit(Digest::new(rand::random()))
        .update_value(30)
        .work_log_id(signer_a.address())
        .build()
        .unwrap();

    // Work log B: 70 work units
    let update_b = LogBuilderJournal::builder()
        .self_image_id(RISC0_POVW_LOG_BUILDER_ID)
        .initial_commit(WorkLog::EMPTY.commit())
        .updated_commit(Digest::new(rand::random()))
        .update_value(70)
        .work_log_id(signer_b.address())
        .build()
        .unwrap();

    let event_a = ctx.post_work_log_update(&signer_a, &update_a, signer_a.address()).await?;
    let event_b = ctx.post_work_log_update(&signer_b, &update_b, signer_b.address()).await?;

    println!("Work log A: {} work units for {:?}", event_a.updateValue, event_a.workLogId);
    println!("Work log B: {} work units for {:?}", event_b.updateValue, event_b.workLogId);

    // Advance time and finalize epoch
    ctx.advance_epochs(U256::ONE).await?;
    let finalized_event = ctx.finalize_epoch().await?;

    // Total work should be 30 + 70 = 100
    assert_eq!(finalized_event.totalWork, U256::from(100));
    println!("Total work in epoch: {}", finalized_event.totalWork);

    // First mint: Filter for work log A only
    let mint_receipt_a = ctx
        .run_mint_with_opts(
            MintOptions::builder()
                .epochs([finalized_event.epoch])
                .work_log_filter([signer_a.address().into()]),
        )
        .await?;
    println!("Mint A transaction succeeded with {} gas used", mint_receipt_a.gas_used);

    // Check balances after first mint
    let balance_a_after_first = ctx.zkc.balanceOf(signer_a.address()).call().await?;
    let balance_b_after_first = ctx.zkc.balanceOf(signer_b.address()).call().await?;
    let epoch_reward = ctx.zkc.getPoVWEmissionsForEpoch(finalized_event.epoch).call().await?;

    // Expected: signer A gets 30% of epoch reward, signer B gets nothing yet
    let expected_a = epoch_reward * U256::from(30) / U256::from(100);
    let tolerance = U256::from(10);

    assert_eq!(
        balance_b_after_first,
        U256::ZERO,
        "Signer B should have received nothing in first mint"
    );
    assert!(
        balance_a_after_first.abs_diff(expected_a) <= tolerance,
        "Signer A should receive ~30% of epoch reward, got {balance_a_after_first}, expected {expected_a}"
    );
    println!("After first mint: A has {balance_a_after_first} tokens, B has {balance_b_after_first} tokens");

    // Second mint: Filter for work log B only
    let mint_receipt_b = ctx
        .run_mint_with_opts(
            MintOptions::builder()
                .epochs([finalized_event.epoch])
                .work_log_filter([signer_b.address().into()]),
        )
        .await?;
    println!("Mint B transaction succeeded with {} gas used", mint_receipt_b.gas_used);

    // Check final balances
    let balance_a_final = ctx.zkc.balanceOf(signer_a.address()).call().await?;
    let balance_b_final = ctx.zkc.balanceOf(signer_b.address()).call().await?;

    // Expected: signer B gets 70% of epoch reward, signer A balance unchanged
    let expected_b = epoch_reward * U256::from(70) / U256::from(100);

    assert_eq!(
        balance_a_final, balance_a_after_first,
        "Signer A balance should be unchanged after second mint"
    );
    assert!(
        balance_b_final.abs_diff(expected_b) <= tolerance,
        "Signer B should receive ~70% of epoch reward, got {balance_b_final}, expected {expected_b}"
    );

    println!(
        "Final balances: A has {balance_a_final} tokens (~30%), B has {balance_b_final} tokens (~70%)"
    );

    Ok(())
}

#[tokio::test]
async fn filter_empty_no_mints_issued() -> anyhow::Result<()> {
    let ctx = test_ctx().await?;

    let initial_epoch = ctx.zkc.getCurrentEpoch().call().await?;
    println!("Initial epoch: {initial_epoch}");

    let signer = PrivateKeySigner::random();

    // Create work log with significant update value
    let update = LogBuilderJournal::builder()
        .self_image_id(RISC0_POVW_LOG_BUILDER_ID)
        .initial_commit(WorkLog::EMPTY.commit())
        .updated_commit(Digest::new(rand::random()))
        .update_value(50)
        .work_log_id(signer.address())
        .build()
        .unwrap();

    let work_log_event = ctx.post_work_log_update(&signer, &update, signer.address()).await?;
    println!(
        "Work log update posted: {} work units for {:?}",
        work_log_event.updateValue, work_log_event.workLogId
    );

    // Advance time and finalize epoch
    ctx.advance_epochs(U256::ONE).await?;
    let finalized_event = ctx.finalize_epoch().await?;

    // The epoch should be finalized with 50 total work
    assert_eq!(finalized_event.epoch, U256::from(initial_epoch));
    assert_eq!(finalized_event.totalWork, U256::from(50));
    println!(
        "EpochFinalized event verified: epoch={}, totalWork={}",
        finalized_event.epoch, finalized_event.totalWork
    );

    // Run mint with empty WorkLogFilter (no work log IDs included)
    let mint_receipt = ctx
        .run_mint_with_opts(
            MintOptions::builder()
                .epochs([finalized_event.epoch])
                .work_log_filter(WorkLogFilter::none()),
        )
        .await?;
    println!(
        "Mint transaction with empty filter succeeded with {} gas used",
        mint_receipt.gas_used
    );

    // Verify no tokens were minted (signer balance should remain zero)
    let signer_balance = ctx.zkc.balanceOf(signer.address()).call().await?;
    assert_eq!(
        signer_balance,
        U256::ZERO,
        "No tokens should be minted when using empty work log filter"
    );
    println!("Verified: signer balance remains zero with empty filter");

    Ok(())
}

#[tokio::test]
async fn reward_cap() -> anyhow::Result<()> {
    let ctx = test_ctx().await?;
    let work_log_signer = PrivateKeySigner::random();
    let value_recipient = PrivateKeySigner::random();

    let initial_epoch = ctx.zkc.getCurrentEpoch().call().await?;
    println!("Initial epoch: {initial_epoch}");

    // Set an epoch reward cap for the recipient.
    let epoch_reward = ctx.zkc.getPoVWEmissionsForEpoch(initial_epoch - U256::ONE).call().await?;
    let capped_epoch_reward = epoch_reward / U256::from(2);
    ctx.zkc_rewards
        .setPoVWRewardCap(work_log_signer.address(), capped_epoch_reward)
        .send()
        .await?
        .watch()
        .await?;

    // Work log controlled by work_log_signer, but rewards should go to value_recipient
    let update = LogBuilderJournal::builder()
        .self_image_id(RISC0_POVW_LOG_BUILDER_ID)
        .initial_commit(WorkLog::EMPTY.commit())
        .updated_commit(Digest::new(rand::random()))
        .update_value(50)
        .work_log_id(work_log_signer.address())
        .build()
        .unwrap();

    let work_log_event =
        ctx.post_work_log_update(&work_log_signer, &update, value_recipient.address()).await?;
    println!("Work log update posted for epoch {}", work_log_event.epochNumber);

    // Verify event has correct value recipient
    assert_eq!(work_log_event.workLogId, work_log_signer.address());
    assert_eq!(work_log_event.valueRecipient, value_recipient.address());

    // Advance time and finalize epoch
    ctx.advance_epochs(U256::ONE).await?;
    let finalized_event = ctx.finalize_epoch().await?;

    assert_eq!(finalized_event.epoch, U256::from(initial_epoch));
    assert_eq!(finalized_event.totalWork, U256::from(50));

    // Run mint calculation
    let mint_receipt = ctx.run_mint().await?;
    println!("Mint transaction succeeded with {} gas used", mint_receipt.gas_used);

    // Check balances - value_recipient should get tokens, not work_log_signer
    let work_log_signer_balance = ctx.zkc.balanceOf(work_log_signer.address()).call().await?;
    let value_recipient_balance = ctx.zkc.balanceOf(value_recipient.address()).call().await?;

    assert_eq!(
        work_log_signer_balance,
        U256::ZERO,
        "Work log signer should not receive any tokens"
    );
    assert_eq!(
        value_recipient_balance, capped_epoch_reward,
        "Value recipient should receive the capped epoch reward"
    );

    Ok(())
}

#[tokio::test]
async fn reward_cap_two_recipients() -> anyhow::Result<()> {
    let ctx = test_ctx().await?;
    let work_log_signer = PrivateKeySigner::random();
    // Create two value recipients. Rewards are doled in the order of the addresses, as an
    // arbitrary way of deciding when rewards are capped.
    let (value_recipient1, value_recipient2) = {
        let mut key1 = PrivateKeySigner::random();
        let mut key2 = PrivateKeySigner::random();
        if key1.address() > key2.address() {
            std::mem::swap(&mut key1, &mut key2);
        }
        (key1, key2)
    };

    let initial_epoch = ctx.zkc.getCurrentEpoch().call().await?;
    println!("Initial epoch: {initial_epoch}");

    // Set an epoch reward cap for the recipient.
    let epoch_reward = ctx.zkc.getPoVWEmissionsForEpoch(initial_epoch - U256::ONE).call().await?;
    let capped_epoch_reward = epoch_reward * U256::from(3) / U256::from(4);
    ctx.zkc_rewards
        .setPoVWRewardCap(work_log_signer.address(), capped_epoch_reward)
        .send()
        .await?
        .watch()
        .await?;

    // Work log controlled by work_log_signer, but rewards should go to value_recipients
    let update1 = LogBuilderJournal::builder()
        .self_image_id(RISC0_POVW_LOG_BUILDER_ID)
        .initial_commit(WorkLog::EMPTY.commit())
        .updated_commit(Digest::new(rand::random()))
        .update_value(50)
        .work_log_id(work_log_signer.address())
        .build()
        .unwrap();

    let work_log_event1 =
        ctx.post_work_log_update(&work_log_signer, &update1, value_recipient1.address()).await?;
    println!(
        "Work log update posted with value recipient {} for epoch {}",
        value_recipient1.address(),
        work_log_event1.epochNumber
    );

    let update2 = LogBuilderJournal::builder()
        .self_image_id(RISC0_POVW_LOG_BUILDER_ID)
        .initial_commit(update1.updated_commit)
        .updated_commit(Digest::new(rand::random()))
        .update_value(50)
        .work_log_id(work_log_signer.address())
        .build()
        .unwrap();

    let work_log_event2 =
        ctx.post_work_log_update(&work_log_signer, &update2, value_recipient2.address()).await?;
    println!(
        "Work log update posted with value recipient {} for epoch {}",
        value_recipient2.address(),
        work_log_event2.epochNumber
    );

    // Advance time and finalize epoch
    ctx.advance_epochs(U256::ONE).await?;
    let finalized_event = ctx.finalize_epoch().await?;

    assert_eq!(finalized_event.epoch, U256::from(initial_epoch));
    assert_eq!(finalized_event.totalWork, U256::from(100));

    // Run mint calculation
    let mint_receipt = ctx.run_mint().await?;
    println!("Mint transaction succeeded with {} gas used", mint_receipt.gas_used);

    // Check balances - value_recipient should get tokens, not work_log_signer
    let work_log_signer_balance = ctx.zkc.balanceOf(work_log_signer.address()).call().await?;
    let value_recipient1_balance = ctx.zkc.balanceOf(value_recipient1.address()).call().await?;
    let value_recipient2_balance = ctx.zkc.balanceOf(value_recipient2.address()).call().await?;

    assert_eq!(
        work_log_signer_balance,
        U256::ZERO,
        "Work log signer should not receive any tokens"
    );
    assert_eq!(
        value_recipient1_balance,
        epoch_reward / U256::from(2),
        "Value recipient {} should receive half the total reward (cap not reached)",
        value_recipient1.address(),
    );
    assert_eq!(
        value_recipient2_balance,
        epoch_reward / U256::from(4),
        "Value recipient {} should receive a quarter of the total reward (cap exceeded)",
        value_recipient2.address(),
    );

    Ok(())
}

#[tokio::test]
async fn reject_incomplete_work_log_processing_across_epochs() -> anyhow::Result<()> {
    let ctx = test_ctx().await?;
    let signer = PrivateKeySigner::random();

    // === First Epoch - Complete Processing ===
    let first_epoch = ctx.zkc.getCurrentEpoch().call().await?;
    println!("First epoch: {first_epoch}");

    let update1 = LogBuilderJournal::builder()
        .self_image_id(RISC0_POVW_LOG_BUILDER_ID)
        .initial_commit(WorkLog::EMPTY.commit())
        .updated_commit(Digest::new(rand::random()))
        .update_value(30)
        .work_log_id(signer.address())
        .build()
        .unwrap();

    ctx.post_work_log_update(&signer, &update1, signer.address()).await?;
    ctx.provider.anvil_mine(Some(1), None).await?; // Force to new block
    println!("Posted first epoch update");

    // Finalize first epoch
    ctx.advance_epochs(U256::ONE).await?;
    ctx.finalize_epoch().await?;

    // === Second Epoch - Incomplete Processing (2 updates, but we'll exclude one) ===
    let second_epoch = ctx.zkc.getCurrentEpoch().call().await?;
    println!("Second epoch: {second_epoch}");

    // Second update (chains from first)
    let update2 = LogBuilderJournal::builder()
        .self_image_id(RISC0_POVW_LOG_BUILDER_ID)
        .initial_commit(update1.updated_commit)
        .updated_commit(Digest::new(rand::random()))
        .update_value(40)
        .work_log_id(signer.address())
        .build()
        .unwrap();

    ctx.post_work_log_update(&signer, &update2, signer.address()).await?;
    ctx.provider.anvil_mine(Some(1), None).await?; // Force to new block
    println!("Posted second epoch first update");

    // Third update (chains from second) - this will be excluded from mint input
    let update3 = LogBuilderJournal::builder()
        .self_image_id(RISC0_POVW_LOG_BUILDER_ID)
        .initial_commit(update2.updated_commit)
        .updated_commit(Digest::new(rand::random()))
        .update_value(50)
        .work_log_id(signer.address())
        .build()
        .unwrap();

    // Capture the block number before posting the third update
    let pre_update3_block = ctx.provider.get_block_number().await?;
    ctx.post_work_log_update(&signer, &update3, signer.address()).await?;
    let excluded_block = pre_update3_block + 1; // The block containing update3
    ctx.provider.anvil_mine(Some(1), None).await?; // Force to new block
    println!("Posted second epoch second update at block {excluded_block} (will be excluded)");

    // Finalize second epoch
    ctx.advance_epochs(U256::ONE).await?;
    ctx.finalize_epoch().await?;

    // === Create Incomplete Mint Input ===
    // This mint input will include all blocks except the one containing update3
    let mint_input = ctx
        .build_mint_input(
            MintOptions::builder()
                .epochs([first_epoch, second_epoch])
                .exclude_blocks([excluded_block]),
        )
        .await?;

    println!("Created mint input excluding block {excluded_block}");

    // === Execute Mint Calculator - Should Fail ===
    let result = execute_mint_calculator_guest(&mint_input);

    assert!(result.is_err(), "Mint should fail due to incomplete work log processing");

    let error_msg = result.unwrap_err().to_string();
    println!("Expected error occurred: {error_msg}");

    // Verify it's specifically the completeness check failure
    assert!(
        error_msg.contains("final commit") && error_msg.contains("does not match"),
        "Error should be about commit mismatch, got: {error_msg}"
    );
    Ok(())
}
