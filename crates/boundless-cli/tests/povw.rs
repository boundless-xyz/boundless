// Copyright 2025 RISC Zero, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! Integration tests for PoVW-related CLI commands.

use std::path::Path;

use alloy::{providers::ext::AnvilApi, signers::local::PrivateKeySigner};
use assert_cmd::Command;
use boundless_cli::commands::povw::State;
use boundless_test_utils::povw::{bento_mock::BentoMockServer, make_work_claim, test_ctx};
use predicates::str::contains;
use risc0_povw::PovwLogId;
use risc0_zkvm::{FakeReceipt, GenericReceipt, ReceiptClaim, VerifierContext, WorkClaim};
use tempfile::TempDir;

// NOTE: Tests in this file print the CLI output. Run `cargo test -- --nocapture --test-threads=1` to see it.

/// Test that the PoVW prepare command shows help correctly.
/// This is a smoke test to ensure the command is properly registered and accessible.
#[test]
fn test_prove_update_help() {
    let mut cmd = Command::cargo_bin("boundless").unwrap();

    cmd.args(["povw", "prepare", "--help"])
        .env("NO_COLOR", "1")
        .env("RUST_LOG", "boundless_cli=debug,info")
        .assert()
        .success()
        .stdout(predicates::str::contains("Usage:"))
        .stdout(predicates::str::contains("prepare"))
        .stderr("");
}

#[tokio::test]
async fn prove_update_basic() -> anyhow::Result<()> {
    // 1. Create a temp dir
    let temp_dir = TempDir::new()?;
    let temp_path = temp_dir.path();

    // Generate a random signer and use its address as the work log ID
    let signer = PrivateKeySigner::random();
    let log_id: PovwLogId = signer.address().into();

    // 2. Make a work receipt, encode it, and save it to the temp dir
    let receipt1_path = temp_path.join("receipt1.bin");
    make_fake_work_receipt_file(log_id, 1000, 10, &receipt1_path)?;

    // 3. Run the prepare command to create a new work log with that receipt
    let state_path = temp_path.join("state.bin");
    let mut cmd = Command::cargo_bin("boundless")?;
    cmd.args([
        "povw",
        "prepare",
        "--new",
        &format!("{:#x}", log_id),
        "--state",
        state_path.to_str().unwrap(),
        receipt1_path.to_str().unwrap(),
    ])
    .env("NO_COLOR", "1")
    .env("RUST_LOG", "boundless_cli=debug,info")
    .env("RISC0_DEV_MODE", "1")
    .assert()
    .success();

    // Verify state file was created and is valid.
    State::load(&state_path)
        .await?
        .validate_with_ctx(&VerifierContext::default().with_dev_mode(true))?;

    // 4. Make another receipt and save it to the temp dir
    let receipt2_path = temp_path.join("receipt2.bin");
    make_fake_work_receipt_file(log_id, 2000, 5, &receipt2_path)?;

    // 5. Run the prepare command again to add the new receipt to the log
    let mut cmd = Command::cargo_bin("boundless")?;
    cmd.args([
        "povw",
        "prepare",
        "--state",
        state_path.to_str().unwrap(),
        receipt2_path.to_str().unwrap(),
    ])
    .env("NO_COLOR", "1")
    .env("RUST_LOG", "boundless_cli=debug,info")
    .env("RISC0_DEV_MODE", "1")
    .assert()
    .success();

    State::load(&state_path)
        .await?
        .validate_with_ctx(&VerifierContext::default().with_dev_mode(true))?;

    Ok(())
}

/// End-to-end test that proves a work log update and sends it to a local Anvil chain.
#[tokio::test]
async fn prove_and_send_update() -> anyhow::Result<()> {
    // 1. Set up a local Anvil node with the required contracts
    let ctx = test_ctx().await?;

    // Create a temp dir for our work receipts and state
    let temp_dir = TempDir::new()?;
    let temp_path = temp_dir.path();

    // Use a random signer for the work log (with zero balance)
    let work_log_signer = PrivateKeySigner::random();
    let log_id: PovwLogId = work_log_signer.address().into();

    // Use an Anvil-provided signer for transaction signing (with balance)
    let tx_signer: PrivateKeySigner = ctx.anvil.lock().await.keys()[1].clone().into();

    let receipt_path = temp_path.join("receipt.bin");
    make_fake_work_receipt_file(log_id, 1000, 10, &receipt_path)?;

    // Run prepare to create a work log update
    let state_path = temp_path.join("state.bin");
    let mut cmd = Command::cargo_bin("boundless")?;
    cmd.args([
        "povw",
        "prepare",
        "--new",
        &format!("{:#x}", log_id),
        "--state",
        state_path.to_str().unwrap(),
        receipt_path.to_str().unwrap(),
    ])
    .env("NO_COLOR", "1")
    .env("RUST_LOG", "boundless_cli=debug,info")
    .env("RISC0_DEV_MODE", "1")
    .assert()
    .success();

    // Verify state file was created and is valid.
    State::load(&state_path)
        .await?
        .validate_with_ctx(&VerifierContext::default().with_dev_mode(true))?;

    // 3. Use the submit command to post an update to the PoVW accounting contract
    let mut cmd = Command::cargo_bin("boundless")?;
    cmd.args(["povw", "submit", "--state", state_path.to_str().unwrap()])
        .env("NO_COLOR", "1")
        .env("RUST_LOG", "boundless_cli=debug,info")
        .env("POVW_ACCOUNTING_ADDRESS", format!("{:#x}", ctx.povw_accounting.address()))
        .env("PRIVATE_KEY", format!("{:#x}", tx_signer.to_bytes()))
        .env("RISC0_DEV_MODE", "1")
        .env("RPC_URL", ctx.anvil.lock().await.endpoint_url().as_str())
        .env("POVW_PRIVATE_KEY", format!("{:#x}", work_log_signer.to_bytes()))
        .assert()
        .success()
        // 4. Confirm that the command logs success
        .stdout(contains("Work log update confirmed"))
        .stdout(contains("updated_commit"));

    // Additional verification: Load the state and check that the work log commit matches onchain
    let state = State::load(&state_path).await?;
    state.validate_with_ctx(&VerifierContext::default().with_dev_mode(true))?;
    let expected_commit = state.work_log.commit();
    let onchain_commit = ctx.povw_accounting.workLogCommit(log_id.into()).call().await?;

    assert_eq!(
        bytemuck::cast::<_, [u8; 32]>(expected_commit),
        *onchain_commit,
        "Onchain commit should match the work log commit from state"
    );

    Ok(())
}

/// Test the claim command with multiple epochs of work log updates.
#[tokio::test]
async fn claim_reward_multi_epoch() -> anyhow::Result<()> {
    // Set up a local Anvil node with the required contracts
    let ctx = test_ctx().await?;

    // Create temp dir for receipts and state
    let temp_dir = TempDir::new()?;
    let temp_path = temp_dir.path();

    // Use a random signer for the work log
    let work_log_signer = PrivateKeySigner::random();
    let log_id: PovwLogId = work_log_signer.address().into();

    // Use a different address as the value recipient
    let value_recipient = PrivateKeySigner::random().address();

    // Use an Anvil-provided signer for transaction signing (with balance)
    let tx_signer: PrivateKeySigner = ctx.anvil.lock().await.keys()[1].clone().into();

    let state_path = temp_path.join("state.bin");
    let work_values = [100u64, 200u64, 150u64]; // Different work values for each epoch

    // Loop: Create three updates across three epochs
    for (i, &work_value) in work_values.iter().enumerate() {
        println!("Creating update {} with work value {}", i + 1, work_value);

        // Create a work receipt for this epoch
        let receipt_path = temp_path.join(format!("receipt_{}.bin", i + 1));
        make_fake_work_receipt_file(log_id, work_value, 10, &receipt_path)?;

        // Create or update the work log
        let mut cmd = Command::cargo_bin("boundless")?;
        let log_id_str = format!("{:#x}", log_id);
        let cmd_args = if i == 0 {
            // First update: create new work log
            vec![
                "povw",
                "prepare",
                "--new",
                &log_id_str,
                "--state",
                state_path.to_str().unwrap(),
                receipt_path.to_str().unwrap(),
            ]
        } else {
            // Subsequent updates: update existing work log (CLI overwrites same state file)
            vec![
                "povw",
                "prepare",
                "--state",
                state_path.to_str().unwrap(),
                receipt_path.to_str().unwrap(),
            ]
        };

        let result = cmd
            .args(cmd_args)
            .env("NO_COLOR", "1")
            .env("RUST_LOG", "boundless_cli=debug,info")
            .env("RISC0_DEV_MODE", "1")
            .assert()
            .success();

        println!(
            "prepare command output:\n{}",
            String::from_utf8_lossy(&result.get_output().stdout)
        );

        // Send the update to the blockchain
        let mut cmd = Command::cargo_bin("boundless")?;
        cmd.args([
            "povw",
            "submit",
            "--state",
            state_path.to_str().unwrap(),
            "--value-recipient",
            &format!("{:#x}", value_recipient),
        ])
        .env("NO_COLOR", "1")
        .env("RUST_LOG", "boundless_cli=debug,info")
        .env("POVW_ACCOUNTING_ADDRESS", format!("{:#x}", ctx.povw_accounting.address()))
        .env("PRIVATE_KEY", format!("{:#x}", tx_signer.to_bytes()))
        .env("RISC0_DEV_MODE", "1")
        .env("RPC_URL", ctx.anvil.lock().await.endpoint_url().as_str())
        .env("POVW_PRIVATE_KEY", format!("{:#x}", work_log_signer.to_bytes()));

        let result = cmd.assert().success().stdout(contains("Work log update confirmed"));

        println!(
            "submit command output:\n{}",
            String::from_utf8_lossy(&result.get_output().stdout)
        );

        // Advance to next epoch after each update
        ctx.advance_epochs(alloy::primitives::U256::from(1)).await?;
    }

    // Finalize the current epoch to make rewards claimable
    ctx.finalize_epoch().await?;
    ctx.provider.anvil_mine(Some(1), None).await?;

    // Run the claim command to mint the accumulated rewards
    println!("Running claim command");
    let mut cmd = Command::cargo_bin("boundless")?;
    cmd.args(["povw", "claim", "--log-id", &format!("{:#x}", log_id)])
        .env("NO_COLOR", "1")
        .env("RUST_LOG", "boundless_cli=debug,info")
        .env("POVW_ACCOUNTING_ADDRESS", format!("{:#x}", ctx.povw_accounting.address()))
        .env("POVW_MINT_ADDRESS", format!("{:#x}", ctx.povw_mint.address()))
        .env("ZKC_ADDRESS", format!("{:#x}", ctx.zkc.address()))
        .env("VEZKC_ADDRESS", format!("{:#x}", ctx.zkc_rewards.address()))
        .env("PRIVATE_KEY", format!("{:#x}", tx_signer.to_bytes()))
        .env("RISC0_DEV_MODE", "1")
        .env("RPC_URL", ctx.anvil.lock().await.endpoint_url().as_str());

    let result = cmd.assert().success().stdout(contains("Reward claim completed"));
    println!("claim command output:\n{}", String::from_utf8_lossy(&result.get_output().stdout));

    // Verify that tokens were minted to the value recipient (not the work log signer)
    let final_balance = ctx.zkc.balanceOf(value_recipient).call().await?;

    // The value recipient should have received rewards for all the work across the epochs
    assert!(
        final_balance > alloy::primitives::U256::ZERO,
        "Value recipient should have received tokens"
    );
    println!("✓ Multi-epoch claim test completed. Final balance: {}", final_balance);

    Ok(())
}

/// Test that if mint is called when one update is in a finalized epoch, and a second update is in
/// an unfinalized epoch, that the process succeeds but provides a warning about the skipped epoch.
#[tokio::test]
async fn claim_on_partially_finalized_epochs() -> anyhow::Result<()> {
    // Set up a local Anvil node with the required contracts
    let ctx = test_ctx().await?;

    // Create temp dir for receipts and state
    let temp_dir = TempDir::new()?;
    let temp_path = temp_dir.path();

    // Use a random signer for the work log
    let work_log_signer = PrivateKeySigner::random();
    let log_id: PovwLogId = work_log_signer.address().into();

    // Use a different address as the value recipient
    let value_recipient = PrivateKeySigner::random().address();

    // Use an Anvil-provided signer for transaction signing (with balance)
    let tx_signer: PrivateKeySigner = ctx.anvil.lock().await.keys()[1].clone().into();

    let state_path = temp_path.join("state.bin");

    // Create a work receipt for the first epoch
    let receipt1_path = temp_path.join("receipt1.bin");
    make_fake_work_receipt_file(log_id, 1000, 10, &receipt1_path)?;

    let mut cmd = Command::cargo_bin("boundless")?;
    let result = cmd
        .args([
            "povw",
            "prepare",
            "--new",
            &format!("{:#x}", log_id),
            "--state",
            state_path.to_str().unwrap(),
            receipt1_path.to_str().unwrap(),
        ])
        .env("NO_COLOR", "1")
        .env("RUST_LOG", "boundless_cli=debug,info")
        .env("RISC0_DEV_MODE", "1")
        .assert()
        .success();

    println!("prepare command output:\n{}", String::from_utf8_lossy(&result.get_output().stdout));

    // Send the update to the blockchain
    let mut cmd = Command::cargo_bin("boundless")?;
    cmd.args([
        "povw",
        "submit",
        "--state",
        state_path.to_str().unwrap(),
        "--value-recipient",
        &format!("{:#x}", value_recipient),
    ])
    .env("NO_COLOR", "1")
    .env("RUST_LOG", "boundless_cli=debug,info")
    .env("POVW_ACCOUNTING_ADDRESS", format!("{:#x}", ctx.povw_accounting.address()))
    .env("PRIVATE_KEY", format!("{:#x}", tx_signer.to_bytes()))
    .env("RISC0_DEV_MODE", "1")
    .env("RPC_URL", ctx.anvil.lock().await.endpoint_url().as_str())
    .env("POVW_PRIVATE_KEY", format!("{:#x}", work_log_signer.to_bytes()));

    let result = cmd.assert().success().stdout(contains("Work log update confirmed"));

    println!("submit command output:\n{}", String::from_utf8_lossy(&result.get_output().stdout));

    // Advance to next epoch after the first update and finalize.
    ctx.advance_epochs(alloy::primitives::U256::from(1)).await?;
    ctx.finalize_epoch().await?;
    ctx.provider.anvil_mine(Some(1), None).await?;

    // Create a work receipt for the second epoch
    let receipt2_path = temp_path.join("receipt2.bin");
    make_fake_work_receipt_file(log_id, 2000, 20, &receipt2_path)?;

    let mut cmd = Command::cargo_bin("boundless")?;
    let result = cmd
        .args([
            "povw",
            "prepare",
            "--state",
            state_path.to_str().unwrap(),
            receipt2_path.to_str().unwrap(),
        ])
        .env("NO_COLOR", "1")
        .env("RUST_LOG", "boundless_cli=debug,info")
        .env("RISC0_DEV_MODE", "1")
        .assert()
        .success();

    println!("prepare command output:\n{}", String::from_utf8_lossy(&result.get_output().stdout));

    // Send the update to the blockchain
    let mut cmd = Command::cargo_bin("boundless")?;
    cmd.args([
        "povw",
        "submit",
        "--state",
        state_path.to_str().unwrap(),
        "--value-recipient",
        &format!("{:#x}", value_recipient),
    ])
    .env("NO_COLOR", "1")
    .env("RUST_LOG", "boundless_cli=debug,info")
    .env("POVW_ACCOUNTING_ADDRESS", format!("{:#x}", ctx.povw_accounting.address()))
    .env("PRIVATE_KEY", format!("{:#x}", tx_signer.to_bytes()))
    .env("RISC0_DEV_MODE", "1")
    .env("RPC_URL", ctx.anvil.lock().await.endpoint_url().as_str())
    .env("POVW_PRIVATE_KEY", format!("{:#x}", work_log_signer.to_bytes()));

    let result = cmd.assert().success().stdout(contains("Work log update confirmed"));

    println!("submit command output:\n{}", String::from_utf8_lossy(&result.get_output().stdout));

    // Advance to next epoch after second update; do not finalize.
    ctx.advance_epochs(alloy::primitives::U256::from(1)).await?;
    ctx.provider.anvil_mine(Some(1), None).await?;

    // Run the claim command to mint the rewards for the first epoch.
    // Will warn about the second epoch.
    println!("Running claim command");
    let mut cmd = Command::cargo_bin("boundless")?;
    cmd.args(["povw", "claim", "--log-id", &format!("{:#x}", log_id)])
        .env("NO_COLOR", "1")
        .env("RUST_LOG", "boundless_cli=debug,info")
        .env("POVW_ACCOUNTING_ADDRESS", format!("{:#x}", ctx.povw_accounting.address()))
        .env("POVW_MINT_ADDRESS", format!("{:#x}", ctx.povw_mint.address()))
        .env("ZKC_ADDRESS", format!("{:#x}", ctx.zkc.address()))
        .env("VEZKC_ADDRESS", format!("{:#x}", ctx.zkc_rewards.address()))
        .env("PRIVATE_KEY", format!("{:#x}", tx_signer.to_bytes()))
        .env("RISC0_DEV_MODE", "1")
        .env("RPC_URL", ctx.anvil.lock().await.endpoint_url().as_str());

    let result = cmd
        .assert()
        .success()
        .stdout(contains("Reward claim completed"))
        .stdout(contains("Skipping update in epoch"));
    println!("claim command output:\n{}", String::from_utf8_lossy(&result.get_output().stdout));

    // Verify that tokens were minted to the value recipient (not the work log signer)
    let final_balance = ctx.zkc.balanceOf(value_recipient).call().await?;

    // The value recipient should have received rewards for all the work across the epochs
    assert!(
        final_balance > alloy::primitives::U256::ZERO,
        "Value recipient should have received tokens"
    );
    Ok(())
}

/// Make a fake work receipt with the given log ID and a random job number, encode it, and save it to a file.
fn make_fake_work_receipt_file(
    log_id: PovwLogId,
    value: u64,
    segments: u32,
    path: impl AsRef<Path>,
) -> anyhow::Result<()> {
    let work_claim = make_work_claim((log_id, rand::random()), segments, value)?; // 10 segments, 1000 value
    let work_receipt: GenericReceipt<WorkClaim<ReceiptClaim>> = FakeReceipt::new(work_claim).into();
    std::fs::write(path.as_ref(), bincode::serialize(&work_receipt)?)?;
    Ok(())
}

/// Test prepare command with Bento API integration
#[tokio::test]
async fn prove_update_from_bento() -> anyhow::Result<()> {
    // 1. Set up the mock Bento server
    let bento_server = BentoMockServer::new().await;
    let bento_url = bento_server.base_url();

    // Create a temp dir for state file
    let temp_dir = TempDir::new()?;
    let temp_path = temp_dir.path();
    let state_path = temp_path.join("state.bin");

    // Generate a work log ID
    let signer = PrivateKeySigner::random();
    let log_id: PovwLogId = signer.address().into();

    // PHASE 1: Add one receipt and run prepare
    tracing::info!("=== Phase 1: Testing with single receipt ===");

    // Add first work receipt to Bento
    let work_claim_1 = make_work_claim((log_id, rand::random()), 10, 1000)?;
    let work_receipt_1: GenericReceipt<WorkClaim<ReceiptClaim>> =
        FakeReceipt::new(work_claim_1).into();
    let receipt_id_1 = bento_server.add_work_receipt(&work_receipt_1)?;
    tracing::info!("Added receipt 1 with ID: {}", receipt_id_1);

    // Run prepare with --from-bento to create new work log
    let mut cmd = Command::cargo_bin("boundless")?;
    let result = cmd
        .args([
            "povw",
            "prepare",
            "--new",
            &format!("{:#x}", log_id),
            "--state",
            state_path.to_str().unwrap(),
            "--from-bento",
            "--from-bento-url",
            &bento_url,
        ])
        .env("NO_COLOR", "1")
        .env("RUST_LOG", "boundless_cli=debug,info")
        .env("RISC0_DEV_MODE", "1")
        .assert()
        .success();

    println!("command output:\n{}", String::from_utf8_lossy(&result.get_output().stdout));

    // Verify state after first update
    let state_1 = State::load(&state_path).await?;
    state_1.validate_with_ctx(&VerifierContext::default().with_dev_mode(true))?;

    // Should have 1 receipt and 1 log builder receipt (1 update)
    assert_eq!(state_1.work_log.jobs.len(), 1, "Should have 1 job in work log");
    assert_eq!(
        state_1.log_builder_receipts.len(),
        1,
        "Should have 1 log builder receipt (1 update)"
    );
    tracing::info!("✓ Phase 1 complete: 1 receipt, 1 update");

    // PHASE 2: Add three more receipts and run prepare again
    tracing::info!("=== Phase 2: Testing with three additional receipts ===");

    // Add three more work receipts to Bento
    let mut receipt_ids = vec![receipt_id_1];
    for i in 2..=4 {
        let work_claim = make_work_claim((log_id, rand::random()), 5, 500 + i * 100)?;
        let work_receipt: GenericReceipt<WorkClaim<ReceiptClaim>> =
            FakeReceipt::new(work_claim).into();
        let receipt_id = bento_server.add_work_receipt(&work_receipt)?;
        receipt_ids.push(receipt_id.clone());
        tracing::info!("Added receipt {} with ID: {}", i, receipt_id);
    }

    assert_eq!(bento_server.receipt_count(), 4, "Should have 4 receipts in mock server");

    // Run prepare again (without --new, updating existing work log)
    let mut cmd = Command::cargo_bin("boundless")?;
    cmd.args([
        "povw",
        "prepare",
        "--state",
        state_path.to_str().unwrap(),
        "--from-bento",
        "--from-bento-url",
        &bento_url,
    ])
    .env("NO_COLOR", "1")
    .env("RUST_LOG", "boundless_cli=debug,info")
    .env("RISC0_DEV_MODE", "1")
    .assert()
    .success();

    // Verify final state
    let state_2 = State::load(&state_path).await?;
    state_2.validate_with_ctx(&VerifierContext::default().with_dev_mode(true))?;

    // Should have 4 receipts total and 2 log builder receipts (2 updates)
    assert_eq!(state_2.work_log.jobs.len(), 4, "Should have 4 jobs in work log");
    assert_eq!(
        state_2.log_builder_receipts.len(),
        2,
        "Should have 2 log builder receipts (2 updates)"
    );

    // Verify that the work log commitment changed (indicating the receipts were processed)
    assert_ne!(
        state_1.work_log.commit(),
        state_2.work_log.commit(),
        "Work log commit should have changed after adding more receipts"
    );

    tracing::info!("✓ Phase 2 complete: 4 total receipts, 2 total updates");
    tracing::info!("✓ Test completed successfully!");

    Ok(())
}

/// Test prepare command when Bento has no new receipts
#[tokio::test]
async fn prove_update_from_bento_no_receipts() -> anyhow::Result<()> {
    // Set up the mock Bento server (empty - no receipts)
    let bento_server = BentoMockServer::new().await;
    let bento_url = bento_server.base_url();

    // Create a temp dir for state file
    let temp_dir = TempDir::new()?;
    let temp_path = temp_dir.path();
    let state_path = temp_path.join("state.bin");

    // Generate a work log ID
    let signer = PrivateKeySigner::random();
    let log_id: PovwLogId = signer.address().into();

    // Run prepare with --from-bento on empty Bento server
    let mut cmd = Command::cargo_bin("boundless")?;
    let result = cmd
        .args([
            "povw",
            "prepare",
            "--new",
            &format!("{:#x}", log_id),
            "--state",
            state_path.to_str().unwrap(),
            "--from-bento",
            "--from-bento-url",
            &bento_url,
        ])
        .env("NO_COLOR", "1")
        .env("RUST_LOG", "boundless_cli=debug,info")
        .env("RISC0_DEV_MODE", "1")
        .assert();

    // Should succeed with message about no receipts to process
    let result = result.success().stdout(predicates::str::contains("No work receipts to process"));

    println!("command output:\n{}", String::from_utf8_lossy(&result.get_output().stdout));

    // Verify that state file was created but is empty (new work log)
    let state = State::load(&state_path).await?;
    assert_eq!(state.log_id, log_id, "State should have the correct log ID");
    assert_eq!(state.work_log.jobs.len(), 0, "Work log should be empty");
    assert_eq!(state.log_builder_receipts.len(), 0, "Should have no log builder receipts");

    tracing::info!("✓ Test completed: Command succeeded with no receipts, created empty state");
    Ok(())
}

/// Test prepare command with work receipts for multiple log IDs
#[tokio::test]
async fn prove_update_from_bento_multiple_log_ids() -> anyhow::Result<()> {
    // Set up the mock Bento server
    let bento_server = BentoMockServer::new().await;
    let bento_url = bento_server.base_url();

    // Create a temp dir for state file
    let temp_dir = TempDir::new()?;
    let temp_path = temp_dir.path();
    let state_path = temp_path.join("state.bin");

    // Generate two different work log IDs
    let target_log_id: PovwLogId = PrivateKeySigner::random().address().into();
    let other_log_id: PovwLogId = PrivateKeySigner::random().address().into();

    tracing::info!("=== Testing prepare with multiple log IDs ===");
    tracing::info!("Target log ID: {:#x}", target_log_id);
    tracing::info!("Other log ID: {:#x}", other_log_id);

    // Add receipts for the target log ID (should be included)
    let target_receipt_1 = {
        let work_claim = make_work_claim((target_log_id, rand::random()), 10, 1000)?;
        let work_receipt = FakeReceipt::new(work_claim).into();
        bento_server.add_work_receipt(&work_receipt)?
    };

    let target_receipt_2 = {
        let work_claim = make_work_claim((target_log_id, rand::random()), 5, 2000)?;
        let work_receipt = FakeReceipt::new(work_claim).into();
        bento_server.add_work_receipt(&work_receipt)?
    };

    // Add receipts for the other log ID (should be skipped)
    let other_receipt_1 = {
        let work_claim = make_work_claim((other_log_id, rand::random()), 8, 1500)?;
        let work_receipt = FakeReceipt::new(work_claim).into();
        bento_server.add_work_receipt(&work_receipt)?
    };

    let other_receipt_2 = {
        let work_claim = make_work_claim((other_log_id, rand::random()), 6, 1800)?;
        let work_receipt = FakeReceipt::new(work_claim).into();
        bento_server.add_work_receipt(&work_receipt)?
    };

    tracing::info!("Added 2 target receipts: {} {}", target_receipt_1, target_receipt_2);
    tracing::info!("Added 2 other receipts: {} {}", other_receipt_1, other_receipt_2);
    assert_eq!(bento_server.receipt_count(), 4, "Should have 4 total receipts in mock server");

    // Run prepare with --from-bento for the target log ID
    let mut cmd = Command::cargo_bin("boundless")?;
    let result = cmd
        .args([
            "povw",
            "prepare",
            "--new",
            &format!("{:#x}", target_log_id),
            "--state",
            state_path.to_str().unwrap(),
            "--from-bento",
            "--from-bento-url",
            &bento_url,
            "--allow-partial-update",
        ])
        .env("NO_COLOR", "1")
        .env("RUST_LOG", "boundless_cli=debug,info")
        .env("RISC0_DEV_MODE", "1")
        .assert();

    // Should succeed and log warnings about skipping other log ID
    let result =
        result.success().stdout(predicates::str::contains("Skipping receipts with log ID"));

    println!("command output:\n{}", String::from_utf8_lossy(&result.get_output().stdout));

    // Verify state was created with only the target receipts
    let state = State::load(&state_path).await?;
    state.validate_with_ctx(&VerifierContext::default().with_dev_mode(true))?;

    // Should have exactly 2 jobs (from target log ID) and 1 log builder receipt
    assert_eq!(state.work_log.jobs.len(), 2, "Should have 2 jobs from target log ID only");
    assert_eq!(state.log_builder_receipts.len(), 1, "Should have 1 log builder receipt");
    assert_eq!(state.log_id, target_log_id, "State should have the correct log ID");

    tracing::info!("✓ Test completed: Correctly processed 2 target receipts, skipped 2 others");
    Ok(())
}
