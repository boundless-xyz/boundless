// Copyright 2026 Boundless Foundation, Inc.
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

//! Integration tests for mining rewards commands.

use alloy::{providers::ext::AnvilApi, signers::local::PrivateKeySigner};
use boundless_cli::commands::rewards::State;
use boundless_test_utils::povw::{bento_mock::BentoMockServer, make_work_claim, test_ctx};
use predicates::str::contains;
use risc0_povw::PovwLogId;
use risc0_zkvm::{FakeReceipt, GenericReceipt, ReceiptClaim, VerifierContext, WorkClaim};
use tempfile::TempDir;

use crate::rewards::{cli_cmd, make_fake_work_receipt_file, RewardsEnv};

// NOTE: Tests in this file print the CLI output. Run `cargo test -- --nocapture --test-threads=1` to see it.

/// Test that the prepare-mining command shows help correctly.
/// This is a smoke test to ensure the command is properly registered and accessible.
#[test]
fn test_prepare_povw_help() {
    let mut cmd = cli_cmd().unwrap();

    cmd.args(["rewards", "prepare-mining", "--help"])
        .env("NO_COLOR", "1")
        .env("RUST_LOG", "boundless_cli=debug,info")
        .assert()
        .success()
        .stdout(contains("Usage:"))
        .stdout(contains("prepare-mining"))
        .stderr("");
}

/// Test that the submit-mining command shows help correctly.
#[test]
fn test_submit_povw_help() {
    let mut cmd = cli_cmd().unwrap();

    cmd.args(["rewards", "submit-mining", "--help"])
        .env("NO_COLOR", "1")
        .assert()
        .success()
        .stdout(contains("Usage:"))
        .stdout(contains("submit-mining"))
        .stderr("");
}

/// Test that the claim-mining-rewards command shows help correctly.
#[test]
fn test_claim_povw_rewards_help() {
    let mut cmd = cli_cmd().unwrap();

    cmd.args(["rewards", "claim-mining-rewards", "--help"])
        .env("NO_COLOR", "1")
        .assert()
        .success()
        .stdout(contains("Usage:"))
        .stdout(contains("claim-mining-rewards"))
        .stderr("");
}

/// Test that the inspect-mining-state command shows help correctly.
#[test]
fn test_inspect_povw_state_help() {
    let mut cmd = cli_cmd().unwrap();

    cmd.args(["rewards", "inspect-mining-state", "--help"])
        .env("NO_COLOR", "1")
        .assert()
        .success()
        .stdout(contains("Usage:"))
        .stdout(contains("inspect-mining-state"))
        .stderr("");
}

/// Basic test for prepare-mining without chain interaction
#[tokio::test]
async fn test_prepare_povw_basic() -> anyhow::Result<()> {
    // 1. Create a temp dir
    let temp_dir = TempDir::new()?;
    let temp_path = temp_dir.path();

    // Generate a random signer and use its address as the work log ID
    let signer = PrivateKeySigner::random();
    let log_id: PovwLogId = signer.address().into();

    // 2. Make a work receipt, encode it, and save it to the temp dir
    let receipt1_path = temp_path.join("receipt1.bin");
    make_fake_work_receipt_file(log_id, 1000, 10, &receipt1_path)?;

    // 3. Create state file and run the prepare-mining command to add the receipt
    let state_path = temp_path.join("state.bin");
    State::new(log_id).save(&state_path)?;

    let mut cmd = cli_cmd()?;
    cmd.args([
        "rewards",
        "prepare-mining",
        "--state-file",
        state_path.to_str().unwrap(),
        "--work-receipt-files",
        receipt1_path.to_str().unwrap(),
    ])
    .env("NO_COLOR", "1")
    .env("RUST_LOG", "boundless_cli=debug,info")
    .env("RISC0_DEV_MODE", "1")
    .assert()
    .success();

    // Verify state file was created and is valid
    let state = State::load(&state_path).await?;
    state.validate_with_ctx(&VerifierContext::default().with_dev_mode(true))?;
    assert_eq!(state.log_id, log_id);
    assert_eq!(state.work_log.jobs.len(), 1, "Should have 1 job in work log");

    // 4. Make another receipt and save it to the temp dir
    let receipt2_path = temp_path.join("receipt2.bin");
    make_fake_work_receipt_file(log_id, 2000, 5, &receipt2_path)?;

    // 5. Run the prepare-mining command again to add the new receipt to the log
    let mut cmd = cli_cmd()?;
    cmd.args([
        "rewards",
        "prepare-mining",
        "--state-file",
        state_path.to_str().unwrap(),
        "--work-receipt-files",
        receipt2_path.to_str().unwrap(),
    ])
    .env("NO_COLOR", "1")
    .env("RUST_LOG", "boundless_cli=debug,info")
    .env("RISC0_DEV_MODE", "1")
    .assert()
    .success();

    // Verify state was updated
    let state = State::load(&state_path).await?;
    state.validate_with_ctx(&VerifierContext::default().with_dev_mode(true))?;
    assert_eq!(state.work_log.jobs.len(), 2, "Should have 2 jobs in work log");

    Ok(())
}

/// Test the inspect-mining-state command
#[tokio::test]
async fn test_inspect_povw_state() -> anyhow::Result<()> {
    // Create a state file with some content
    let temp_dir = TempDir::new()?;
    let temp_path = temp_dir.path();
    let state_path = temp_path.join("state.bin");

    let signer = PrivateKeySigner::random();
    let log_id: PovwLogId = signer.address().into();

    // Create a receipt
    let receipt_path = temp_path.join("receipt.bin");
    make_fake_work_receipt_file(log_id, 1000, 10, &receipt_path)?;

    // Create state file
    State::new(log_id).save(&state_path)?;

    let mut cmd = cli_cmd()?;
    cmd.args([
        "rewards",
        "prepare-mining",
        "--state-file",
        state_path.to_str().unwrap(),
        "--work-receipt-files",
        receipt_path.to_str().unwrap(),
    ])
    .env("NO_COLOR", "1")
    .env("RUST_LOG", "boundless_cli=debug,info")
    .env("RISC0_DEV_MODE", "1")
    .assert()
    .success();

    // Now inspect the state file
    let mut cmd = cli_cmd()?;
    cmd.args(["rewards", "inspect-mining-state", "--state-file", state_path.to_str().unwrap()])
        .env("NO_COLOR", "1")
        .assert()
        .success()
        .stdout(contains("State Metadata"))
        .stdout(contains("Log ID:"))
        .stdout(contains("Work Log"))
        .stdout(contains("Jobs:"))
        .stdout(contains("1")); // Should show 1 job

    Ok(())
}

/// End-to-end test that proves a work log update and sends it to a local Anvil chain.
#[tokio::test]
async fn test_prepare_and_submit() -> anyhow::Result<()> {
    // 1. Set up a local Anvil node with the required contracts
    let ctx = test_ctx().await?;

    // Create a temp dir for our work receipts and state
    let temp_dir = TempDir::new()?;
    let temp_path = temp_dir.path();

    // Use an Anvil-provided signer for both work log and transactions (has balance)
    let tx_signer: PrivateKeySigner = ctx.anvil.lock().await.keys()[1].clone().into();
    let log_id: PovwLogId = tx_signer.address().into();

    let receipt_path = temp_path.join("receipt.bin");
    make_fake_work_receipt_file(log_id, 1000, 10, &receipt_path)?;

    // Create state file and run prepare-mining to create a work log update
    let state_path = temp_path.join("state.bin");
    State::new(log_id).save(&state_path)?;

    let mut cmd = cli_cmd()?;
    cmd.args([
        "rewards",
        "prepare-mining",
        "--state-file",
        state_path.to_str().unwrap(),
        "--work-receipt-files",
        receipt_path.to_str().unwrap(),
    ])
    .env("NO_COLOR", "1")
    .env("RUST_LOG", "boundless_cli=debug,info")
    .env("RISC0_DEV_MODE", "1")
    .assert()
    .success();

    // Verify state file was created and is valid
    let state = State::load(&state_path).await?;
    state.validate_with_ctx(&VerifierContext::default().with_dev_mode(true))?;

    // 3. Use the submit-mining command to post an update to the PoVW accounting contract
    let env = RewardsEnv::from_test_ctx(&ctx, &tx_signer).await;
    let mut cmd = cli_cmd()?;
    env.apply_to_cmd(&mut cmd);
    cmd.args(["rewards", "submit-mining", "--state-file", state_path.to_str().unwrap()])
        .assert()
        .success()
        // 4. Confirm that the command logs success
        .stdout(contains("Successfully submitted"));

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
async fn test_claim_multi_epoch() -> anyhow::Result<()> {
    // Set up a local Anvil node with the required contracts
    let ctx = test_ctx().await?;

    // Create temp dir for receipts and state
    let temp_dir = TempDir::new()?;
    let temp_path = temp_dir.path();

    // Use an Anvil-provided signer for both work log and transactions (has balance)
    let tx_signer: PrivateKeySigner = ctx.anvil.lock().await.keys()[1].clone().into();
    let log_id: PovwLogId = tx_signer.address().into();

    // Use a different address as the value recipient
    let value_recipient = PrivateKeySigner::random().address();

    let state_path = temp_path.join("state.bin");
    let work_values = [100u64, 200u64, 150u64]; // Different work values for each epoch

    let env = RewardsEnv::from_test_ctx(&ctx, &tx_signer).await;

    // Create initial state file
    State::new(log_id).save(&state_path)?;

    // Loop: Create three updates across three epochs
    for (i, &work_value) in work_values.iter().enumerate() {
        println!("Creating update {} with work value {}", i + 1, work_value);

        // Create a work receipt for this epoch
        let receipt_path = temp_path.join(format!("receipt_{}.bin", i + 1));
        make_fake_work_receipt_file(log_id, work_value, 10, &receipt_path)?;

        // Update the work log
        let mut cmd = cli_cmd()?;
        let result = cmd
            .args([
                "rewards",
                "prepare-mining",
                "--state-file",
                state_path.to_str().unwrap(),
                "--work-receipt-files",
                receipt_path.to_str().unwrap(),
            ])
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
        let mut cmd = cli_cmd()?;
        env.apply_to_cmd(&mut cmd);
        cmd.args([
            "rewards",
            "submit-mining",
            "--state-file",
            state_path.to_str().unwrap(),
            "--recipient",
            &format!("{:#x}", value_recipient),
        ]);

        let result = cmd.assert().success().stdout(contains("Successfully submitted"));

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
    let mut cmd = cli_cmd()?;
    env.apply_to_cmd(&mut cmd);
    cmd.args(["rewards", "claim-mining-rewards", "--reward-address", &format!("{:#x}", log_id)]);

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
async fn test_claim_partial_finalization() -> anyhow::Result<()> {
    // Set up a local Anvil node with the required contracts
    let ctx = test_ctx().await?;

    // Create temp dir for receipts and state
    let temp_dir = TempDir::new()?;
    let temp_path = temp_dir.path();

    // Use an Anvil-provided signer for both work log and transactions (has balance)
    let tx_signer: PrivateKeySigner = ctx.anvil.lock().await.keys()[1].clone().into();
    let log_id: PovwLogId = tx_signer.address().into();

    // Use a different address as the value recipient
    let value_recipient = PrivateKeySigner::random().address();

    let state_path = temp_path.join("state.bin");
    let env = RewardsEnv::from_test_ctx(&ctx, &tx_signer).await;

    // Create initial state file
    State::new(log_id).save(&state_path)?;

    // Create a work receipt for the first epoch
    let receipt1_path = temp_path.join("receipt1.bin");
    make_fake_work_receipt_file(log_id, 1000, 10, &receipt1_path)?;

    let mut cmd = cli_cmd()?;
    let result = cmd
        .args([
            "rewards",
            "prepare-mining",
            "--state-file",
            state_path.to_str().unwrap(),
            "--work-receipt-files",
            receipt1_path.to_str().unwrap(),
        ])
        .env("NO_COLOR", "1")
        .env("RUST_LOG", "boundless_cli=debug,info")
        .env("RISC0_DEV_MODE", "1")
        .assert()
        .success();

    println!("prepare command output:\n{}", String::from_utf8_lossy(&result.get_output().stdout));

    // Send the update to the blockchain
    let mut cmd = cli_cmd()?;
    env.apply_to_cmd(&mut cmd);
    cmd.args([
        "rewards",
        "submit-mining",
        "--state-file",
        state_path.to_str().unwrap(),
        "--recipient",
        &format!("{:#x}", value_recipient),
    ]);

    let result = cmd.assert().success().stdout(contains("Successfully submitted"));

    println!("submit command output:\n{}", String::from_utf8_lossy(&result.get_output().stdout));

    // Advance to next epoch after the first update and finalize
    ctx.advance_epochs(alloy::primitives::U256::from(1)).await?;
    ctx.finalize_epoch().await?;
    ctx.provider.anvil_mine(Some(1), None).await?;

    // Create a work receipt for the second epoch
    let receipt2_path = temp_path.join("receipt2.bin");
    make_fake_work_receipt_file(log_id, 2000, 20, &receipt2_path)?;

    let mut cmd = cli_cmd()?;
    let result = cmd
        .args([
            "rewards",
            "prepare-mining",
            "--state-file",
            state_path.to_str().unwrap(),
            "--work-receipt-files",
            receipt2_path.to_str().unwrap(),
        ])
        .env("NO_COLOR", "1")
        .env("RUST_LOG", "boundless_cli=debug,info")
        .env("RISC0_DEV_MODE", "1")
        .assert()
        .success();

    println!("prepare command output:\n{}", String::from_utf8_lossy(&result.get_output().stdout));

    // Send the update to the blockchain
    let mut cmd = cli_cmd()?;
    env.apply_to_cmd(&mut cmd);
    cmd.args([
        "rewards",
        "submit-mining",
        "--state-file",
        state_path.to_str().unwrap(),
        "--recipient",
        &format!("{:#x}", value_recipient),
    ]);

    let result = cmd.assert().success().stdout(contains("Successfully submitted"));

    println!("submit command output:\n{}", String::from_utf8_lossy(&result.get_output().stdout));

    // Advance to next epoch after second update; do not finalize
    ctx.advance_epochs(alloy::primitives::U256::from(1)).await?;
    ctx.provider.anvil_mine(Some(1), None).await?;

    // Run the claim command to mint the rewards for the first epoch
    // Will warn about the second epoch
    println!("Running claim command");
    let mut cmd = cli_cmd()?;
    env.apply_to_cmd(&mut cmd);
    cmd.args(["rewards", "claim-mining-rewards", "--reward-address", &format!("{:#x}", log_id)]);

    let result = cmd
        .assert()
        .success()
        .stdout(contains("Reward claim completed"))
        .stdout(contains("Skipping update in epoch"));
    println!("claim command output:\n{}", String::from_utf8_lossy(&result.get_output().stdout));

    // Verify that tokens were minted to the value recipient (not the work log signer)
    let final_balance = ctx.zkc.balanceOf(value_recipient).call().await?;

    // The value recipient should have received rewards for the work in the first epoch
    assert!(
        final_balance > alloy::primitives::U256::ZERO,
        "Value recipient should have received tokens"
    );
    Ok(())
}

/// Test prepare command with Bento API integration
#[tokio::test]
async fn test_prepare_from_bento() -> anyhow::Result<()> {
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

    // Create initial state file
    State::new(log_id).save(&state_path)?;

    // PHASE 1: Add one receipt and run prepare
    tracing::info!("=== Phase 1: Testing with single receipt ===");

    // Add first work receipt to Bento
    let work_claim_1 = make_work_claim((log_id, rand::random()), 10, 1000)?;
    let work_receipt_1: GenericReceipt<WorkClaim<ReceiptClaim>> =
        FakeReceipt::new(work_claim_1).into();
    let receipt_id_1 = bento_server.add_work_receipt(&work_receipt_1)?;
    tracing::info!("Added receipt 1 with ID: {}", receipt_id_1);

    // Run prepare with --work-receipt-bento-api-url to fetch from Bento
    let mut cmd = cli_cmd()?;
    let result = cmd
        .args([
            "rewards",
            "prepare-mining",
            "--state-file",
            state_path.to_str().unwrap(),
            "--work-receipt-bento-api-url",
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

    // Run prepare again to fetch new receipts from Bento
    let mut cmd = cli_cmd()?;
    cmd.args([
        "rewards",
        "prepare-mining",
        "--state-file",
        state_path.to_str().unwrap(),
        "--work-receipt-bento-api-url",
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
async fn test_prepare_from_bento_no_receipts() -> anyhow::Result<()> {
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

    // Create initial state file
    State::new(log_id).save(&state_path)?;

    // Run prepare with --work-receipt-bento-api-url on empty Bento server
    let mut cmd = cli_cmd()?;
    let result = cmd
        .args([
            "rewards",
            "prepare-mining",
            "--state-file",
            state_path.to_str().unwrap(),
            "--work-receipt-bento-api-url",
            &bento_url,
        ])
        .env("NO_COLOR", "1")
        .env("RUST_LOG", "boundless_cli=debug,info")
        .env("RISC0_DEV_MODE", "1")
        .assert();

    // Should succeed with message about no receipts to process
    let result = result.success().stdout(contains("No new receipts"));

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
async fn test_prepare_from_bento_multiple_log_ids() -> anyhow::Result<()> {
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

    // Create initial state file
    State::new(target_log_id).save(&state_path)?;

    // Run prepare with --work-receipt-bento-api-url for the target log ID
    let mut cmd = cli_cmd()?;
    let result = cmd
        .args([
            "rewards",
            "prepare-mining",
            "--state-file",
            state_path.to_str().unwrap(),
            "--work-receipt-bento-api-url",
            &bento_url,
            "--allow-partial-update",
        ])
        .env("NO_COLOR", "1")
        .env("RUST_LOG", "boundless_cli=debug,info")
        .env("RISC0_DEV_MODE", "1")
        .assert();

    // Should succeed and log warnings about skipping other log ID
    let result = result.success().stdout(contains("receipts associated with"));

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

/// Test that prepare-mining creates a backup when updating an existing state file
#[tokio::test]
async fn test_prepare_creates_backup() -> anyhow::Result<()> {
    // Create a temp dir for the state file
    let temp_dir = TempDir::new()?;
    let temp_path = temp_dir.path();

    // Generate a random signer
    let signer = PrivateKeySigner::random();
    let log_id: PovwLogId = signer.address().into();

    // Create first receipt
    let receipt1_path = temp_path.join("receipt1.bin");
    make_fake_work_receipt_file(log_id, 1000, 10, &receipt1_path)?;

    // Create initial state
    let state_path = temp_path.join("state.bin");
    State::new(log_id).save(&state_path)?;

    let mut cmd = cli_cmd()?;
    cmd.args([
        "rewards",
        "prepare-mining",
        "--state-file",
        state_path.to_str().unwrap(),
        "--work-receipt-files",
        receipt1_path.to_str().unwrap(),
    ])
    .env("NO_COLOR", "1")
    .env("RUST_LOG", "boundless_cli=debug,info")
    .env("RISC0_DEV_MODE", "1")
    .assert()
    .success();

    // Create second receipt
    let receipt2_path = temp_path.join("receipt2.bin");
    make_fake_work_receipt_file(log_id, 2000, 5, &receipt2_path)?;

    // Sleep to ensure different timestamp for backup filename
    tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;

    // Get home directory for backup location
    let home = dirs::home_dir().expect("Could not determine home directory");
    let backup_dir = home.join(".boundless").join("backups");

    // Count existing backup files before update
    let before_count = std::fs::read_dir(&backup_dir)
        .map(|entries| {
            entries
                .filter_map(|e| e.ok())
                .filter(|e| e.file_name().to_string_lossy().contains(&format!("{:x}", log_id)))
                .count()
        })
        .unwrap_or(0);

    // Update state file (should create backup)
    let mut cmd = cli_cmd()?;
    let result = cmd
        .args([
            "rewards",
            "prepare-mining",
            "--state-file",
            state_path.to_str().unwrap(),
            "--work-receipt-files",
            receipt2_path.to_str().unwrap(),
        ])
        .env("NO_COLOR", "1")
        .env("RUST_LOG", "boundless_cli=debug,info")
        .env("RISC0_DEV_MODE", "1")
        .assert()
        .success();

    // Check that output mentions backup
    let output = String::from_utf8_lossy(&result.get_output().stdout);
    assert!(output.contains("Saved backup to"), "Should mention backup creation");

    // Count backup files after update
    let after_count = std::fs::read_dir(&backup_dir)
        .map(|entries| {
            entries
                .filter_map(|e| e.ok())
                .filter(|e| e.file_name().to_string_lossy().contains(&format!("{:x}", log_id)))
                .count()
        })
        .unwrap_or(0);

    assert_eq!(after_count, before_count + 1, "Should have created one new backup file");

    // Verify updated state has 2 jobs
    let state = State::load(&state_path).await?;
    assert_eq!(state.work_log.jobs.len(), 2, "Updated state should have 2 jobs");

    Ok(())
}
