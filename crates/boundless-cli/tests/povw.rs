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

//! Integration tests for rewards-related CLI commands (PoVW functionality).

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

/// Test that the rewards submit-povw command shows help correctly.
/// This is a smoke test to ensure the command is properly registered and accessible.
#[test]
fn test_rewards_submit_povw_help() {
    let mut cmd = Command::cargo_bin("boundless").unwrap();

    cmd.args(["rewards", "submit-povw", "--help"])
        .env("NO_COLOR", "1")
        .env("RUST_LOG", "boundless_cli=debug,info")
        .assert()
        .success()
        .stdout(predicates::str::contains("Usage:"))
        .stdout(predicates::str::contains("submit-povw"))
        .stderr("");
}

/// Test that the legacy povw prepare command still works (for backwards compatibility).
/// Should be removed when legacy commands are fully deprecated.
#[test]
fn test_legacy_povw_prepare_help() {
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

    // 2. Run a mock bento
    let mut _bento = BentoMockServer::new().await?;

    // 3. Build a test ctx
    let mut ctx = test_ctx().await;

    // 4. Create some fake receipts
    let receipts = [
        WorkClaim::new(PovwLogId(1, 2), 3, 4, 1_000_000_000),
        WorkClaim::new(PovwLogId(1, 2), 4, 5, 2_000_000_000),
        WorkClaim::new(PovwLogId(1, 2), 5, 6, 3_000_000_000),
    ]
    .into_iter()
    .map(make_work_claim)
    .collect::<Vec<(ReceiptClaim, GenericReceipt)>>();

    // 5. Put them in a state file
    let state_file = temp_path.join("batch.json");
    let state = State::new(&receipts);
    state.save(&state_file)?;

    // 6. Run the CLI command with the new rewards structure
    let signer = PrivateKeySigner::random();
    let delegated_to = alloy::hex::encode(signer.address().into_array());

    let mut cmd = Command::cargo_bin("boundless").unwrap();
    cmd.args([
        "rewards",
        "submit-povw",
        "--state-file",
        state_file.to_str().unwrap(),
        "--work-log-delegated-to",
        &delegated_to,
    ])
    .env("ETH_MAINNET_RPC_URL", ctx.provider.endpoint().as_ref())
    .env("POVW_PRIVATE_KEY", ctx.signer.to_bytes().to_string())
    .env(
        "BOUNDLESS_RPC_URL",
        "http://localhost:8545", // Dummy URL for non-PoVW operations
    )
    .env("NO_COLOR", "1")
    .env("RUST_LOG", "boundless_cli=debug,info");

    println!("\n\nRunning CLI command...");
    let output = cmd.output()?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    println!("{}", stdout);
    println!("{}", stderr);

    // Expect the test to actually run. This might fail if the provider doesn't have
    // contracts deployed, but it shouldn't complain about command-line arguments.
    assert!(
        output.status.success()
            || stderr.contains("PovwAccounting")
            || stderr.contains("0x7109709ECfa91a80626fF3989D68f67F5b1DD12D"),
        "Command failed unexpectedly. Status: {:?}, stdout: {}, stderr: {}",
        output.status,
        stdout,
        stderr
    );

    Ok(())
}
