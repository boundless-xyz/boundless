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

//! Integration tests for rewards-related CLI commands (ZKC functionality).

use alloy::{
    primitives::{utils::format_ether, U256},
    signers::local::PrivateKeySigner,
};
use assert_cmd::Command;
use boundless_test_utils::zkc::test_ctx;
use predicates::str::contains;

/// Test the rewards balance-zkc command
#[tokio::test]
async fn test_balance_zkc() -> anyhow::Result<()> {
    // Set up a local Anvil node with the required contracts
    let ctx = test_ctx().await?;

    // Use an Anvil-provided signer for transaction signing (with balance)
    let user: PrivateKeySigner = ctx.anvil.lock().await.keys()[1].clone().into();

    // Fund the user
    let amount = U256::from(1_000_000_000);
    ctx.zkc.initialMint(vec![user.address()], vec![amount]).send().await?.watch().await?;

    // Run balance-zkc with the new rewards command
    let mut cmd = Command::cargo_bin("boundless")?;
    cmd.args(["rewards", "balance-zkc", &format!("{:#x}", user.address())])
        .env("ZKC_ADDRESS", format!("{:#x}", ctx.deployment.zkc_address))
        .env("VEZKC_ADDRESS", format!("{:#x}", ctx.deployment.vezkc_address))
        .env("STAKING_REWARDS_ADDRESS", format!("{:#x}", ctx.deployment.staking_rewards_address))
        .env("ETH_MAINNET_RPC_URL", ctx.anvil.lock().await.endpoint_url().as_str())
        .env("BOUNDLESS_RPC_URL", "http://localhost:8545") // Dummy for non-rewards operations
        .env("NO_COLOR", "1")
        .env("RUST_LOG", "boundless_cli=debug,info")
        .assert()
        .success()
        .stdout(contains(format_ether(amount)));

    Ok(())
}

/// Test the legacy zkc balance-of command (for backwards compatibility)
/// Should be removed when legacy commands are fully deprecated
#[tokio::test]
async fn test_legacy_balance_of() -> anyhow::Result<()> {
    // Set up a local Anvil node with the required contracts
    let ctx = test_ctx().await?;

    // Use an Anvil-provided signer for transaction signing (with balance)
    let user: PrivateKeySigner = ctx.anvil.lock().await.keys()[1].clone().into();

    // Fund the user
    let amount = U256::from(1_000_000_000);
    ctx.zkc.initialMint(vec![user.address()], vec![amount]).send().await?.watch().await?;

    // Run balance-of with legacy command
    let mut cmd = Command::cargo_bin("boundless")?;
    cmd.args(["zkc", "balance-of", &format!("{:#x}", user.address())])
        .env("ZKC_ADDRESS", format!("{:#x}", ctx.deployment.zkc_address))
        .env("VEZKC_ADDRESS", format!("{:#x}", ctx.deployment.vezkc_address))
        .env("STAKING_REWARDS_ADDRESS", format!("{:#x}", ctx.deployment.staking_rewards_address))
        .env("RPC_URL", ctx.anvil.lock().await.endpoint_url().as_str())
        .env("NO_COLOR", "1")
        .env("RUST_LOG", "boundless_cli=debug,info")
        .assert()
        .success()
        .stdout(contains(format_ether(amount)));

    Ok(())
}

/// Test the rewards stake-zkc --dry-run command
#[test]
fn test_stake_zkc_dry_run() {
    let mut cmd = Command::cargo_bin("boundless").unwrap();

    cmd.args(["rewards", "stake-zkc", "--amount", "1000000000000000000", "--dry-run"])
        .env("NO_COLOR", "1")
        .env("RUST_LOG", "boundless_cli=debug,info")
        .assert()
        .success()
        .stdout(contains("DRY RUN MODE"))
        .stdout(contains("Estimating staking rewards"));
}

/// Test the rewards list-povw-rewards command help
#[test]
fn test_list_povw_rewards_help() {
    let mut cmd = Command::cargo_bin("boundless").unwrap();

    cmd.args(["rewards", "list-povw-rewards", "--help"])
        .env("NO_COLOR", "1")
        .env("RUST_LOG", "boundless_cli=debug,info")
        .assert()
        .success()
        .stdout(contains("Usage:"))
        .stdout(contains("list-povw-rewards"))
        .stderr("");
}
