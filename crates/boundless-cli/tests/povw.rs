//! Integration tests for PoVW-related CLI commands.

use std::path::Path;

use alloy::signers::local::PrivateKeySigner;
use assert_cmd::Command;
use boundless_cli::commands::povw::load_state;
use boundless_test_utils::povw::{make_work_claim, test_ctx};
use predicates::str::contains;
use risc0_povw::PovwLogId;
use risc0_zkvm::{FakeReceipt, GenericReceipt, ReceiptClaim, WorkClaim};
use tempfile::TempDir;

/// Test that the PoVW prove-update command shows help correctly.
/// This is a smoke test to ensure the command is properly registered and accessible.
#[test]
fn test_prove_update_help() {
    let mut cmd = Command::cargo_bin("boundless").unwrap();

    cmd.args(["povw", "prove-update", "--help"])
        .assert()
        .success()
        .stdout(predicates::str::contains("Usage:"))
        .stdout(predicates::str::contains("prove-update"))
        .stderr("");
}

#[test]
fn prove_update_basic() -> anyhow::Result<()> {
    // 1. Create a temp dir
    let temp_dir = TempDir::new()?;
    let temp_path = temp_dir.path();

    // Generate a random signer and use its address as the work log ID
    let signer = PrivateKeySigner::random();
    let log_id: PovwLogId = signer.address().into();

    // 2. Make a work receipt, encode it, and save it to the temp dir
    let receipt1_path = temp_path.join("receipt1.bin");
    make_fake_work_receipt_file(log_id, 1000, 10, &receipt1_path)?;

    // 3. Run the prove-update command to create a new work log with that receipt
    let state_path = temp_path.join("state.bin");
    let mut cmd = Command::cargo_bin("boundless")?;
    cmd.args([
        "povw",
        "prove-update",
        "--new",
        "--log-id",
        &format!("{:#x}", log_id),
        "--state-out",
        state_path.to_str().unwrap(),
        receipt1_path.to_str().unwrap(),
    ])
    .env("RISC0_DEV_MODE", "1")
    .assert()
    .success();

    // Verify state file was created
    assert!(state_path.exists(), "State file should be created");

    // 4. Make another receipt and save it to the temp dir
    let receipt2_path = temp_path.join("receipt2.bin");
    make_fake_work_receipt_file(log_id, 2000, 5, &receipt2_path)?;

    // 5. Run the prove-update command again to add the new receipt to the log
    let updated_state_path = temp_path.join("updated_state.bin");
    let mut cmd = Command::cargo_bin("boundless")?;
    cmd.args([
        "povw",
        "prove-update",
        "--state-in",
        state_path.to_str().unwrap(),
        "--log-id",
        &format!("{:#x}", log_id),
        "--state-out",
        updated_state_path.to_str().unwrap(),
        receipt2_path.to_str().unwrap(),
    ])
    .env("RISC0_DEV_MODE", "1")
    .assert()
    .success();

    // Verify updated state file was created
    assert!(updated_state_path.exists(), "Updated state file should be created");

    Ok(())
}

/// End-to-end test that proves a work log update and sends it to a local Anvil chain.
#[tokio::test]
async fn prove_and_send_update_end_to_end() -> anyhow::Result<()> {
    // 1. Set up a local Anvil node with the required contracts
    let ctx = test_ctx().await?;

    // Create a temp dir for our work receipts and state
    let temp_dir = TempDir::new()?;
    let temp_path = temp_dir.path();

    // Use a random signer for the work log (with zero balance)
    let work_log_signer = PrivateKeySigner::random();
    let log_id: PovwLogId = work_log_signer.address().into();

    // Use an Anvil-provided signer for transaction signing (with balance)
    let tx_signer: PrivateKeySigner = ctx.anvil.lock().await.keys()[0].clone().into();

    let receipt_path = temp_path.join("receipt.bin");
    make_fake_work_receipt_file(log_id, 1000, 10, &receipt_path)?;

    // Run prove-update to create a work log update
    let state_path = temp_path.join("state.bin");
    let mut cmd = Command::cargo_bin("boundless")?;
    cmd.args([
        "povw",
        "prove-update",
        "--new",
        "--log-id",
        &format!("{:#x}", log_id),
        "--state-out",
        state_path.to_str().unwrap(),
        receipt_path.to_str().unwrap(),
    ])
    .env("RISC0_DEV_MODE", "1")
    .assert()
    .success();

    // Verify state file was created
    assert!(state_path.exists(), "State file should be created");

    // 3. Use the send-update command to post an update to the PoVW accounting contract
    let mut cmd = Command::cargo_bin("boundless")?;
    cmd.args([
        "povw",
        "send-update",
        "--state",
        state_path.to_str().unwrap(),
        &format!("{:#x}", ctx.povw_accounting.address()),
    ])
    .env("RISC0_DEV_MODE", "1")
    .env("WORK_LOG_PRIVATE_KEY", format!("{:#x}", work_log_signer.to_bytes()))
    .env("PRIVATE_KEY", format!("{:#x}", tx_signer.to_bytes()))
    .env("RPC_URL", ctx.anvil.lock().await.endpoint_url().as_str())
    .assert()
    .success()
    // 4. Confirm that the command logs success
    .stdout(contains("Work log updated confirmed"))
    .stdout(contains("updated_commit"));

    // Additional verification: Load the state and check that the work log commit matches onchain
    let state = load_state(&state_path)?;
    let expected_commit = state.work_log.commit();
    let onchain_commit = ctx.povw_accounting.workLogCommit(log_id.into()).call().await?;

    assert_eq!(
        bytemuck::cast::<_, [u8; 32]>(expected_commit),
        *onchain_commit,
        "Onchain commit should match the work log commit from state"
    );

    Ok(())
}

/// Make a fake work receipt with the given log ID and a random job number, encode it, and save it to a file.
fn make_fake_work_receipt_file(log_id: PovwLogId, value: u64, segments: u32, path: impl AsRef<Path>) -> anyhow::Result<()> {
    let work_claim = make_work_claim((log_id, rand::random()), segments, value)?; // 10 segments, 1000 value
    let work_receipt: GenericReceipt<WorkClaim<ReceiptClaim>> = FakeReceipt::new(work_claim).into();
    std::fs::write(path.as_ref(), bincode::serialize(&work_receipt)?)?;
    Ok(())
}
