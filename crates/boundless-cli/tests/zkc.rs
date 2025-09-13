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

//! Integration tests for ZKC-related CLI commands.

use alloy::{
    primitives::{utils::format_ether, U256},
    signers::local::PrivateKeySigner,
};
use assert_cmd::Command;
use boundless_test_utils::zkc::test_ctx;
use predicates::str::contains;

#[tokio::test]
async fn test_balance_of() -> anyhow::Result<()> {
    // Set up a local Anvil node with the required contracts
    let ctx = test_ctx().await?;

    // Use an Anvil-provided signer for transaction signing (with balance)
    let user: PrivateKeySigner = ctx.anvil.lock().await.keys()[1].clone().into();

    // Fund the user
    let amount = U256::from(1_000_000_000);
    ctx.zkc.initialMint(vec![user.address()], vec![amount]).send().await?.watch().await?;

    // Run balance-of
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

#[tokio::test]
async fn test_stake_unstake() -> anyhow::Result<()> {
    // Set up a local Anvil node with the required contracts
    let ctx = test_ctx().await?;

    // Use an Anvil-provided signer for transaction signing (with balance)
    let user: PrivateKeySigner = ctx.anvil.lock().await.keys()[1].clone().into();
    let user_private_key = format!("0x{}", hex::encode(user.to_bytes()));

    // Fund the user
    let amount = U256::from(1_000_000_000);
    let stake_amount = U256::from(500_000_000);
    ctx.zkc.initialMint(vec![user.address()], vec![amount]).send().await?.watch().await?;

    // Run stake
    let mut cmd = Command::cargo_bin("boundless")?;
    cmd.args(["zkc", "stake", "--amount", stake_amount.to_string().as_str()])
        .env("ZKC_ADDRESS", format!("{:#x}", ctx.deployment.zkc_address))
        .env("VEZKC_ADDRESS", format!("{:#x}", ctx.deployment.vezkc_address))
        .env("STAKING_REWARDS_ADDRESS", format!("{:#x}", ctx.deployment.staking_rewards_address))
        .env("RPC_URL", ctx.anvil.lock().await.endpoint_url().as_str())
        .env("PRIVATE_KEY", &user_private_key)
        .env("NO_COLOR", "1")
        .env("RUST_LOG", "boundless_cli=debug,info")
        .assert()
        .success()
        .stdout(contains(format_ether(stake_amount)));

    // Run stake again
    let mut cmd = Command::cargo_bin("boundless")?;
    cmd.args(["zkc", "stake", "--amount", stake_amount.to_string().as_str()])
        .env("ZKC_ADDRESS", format!("{:#x}", ctx.deployment.zkc_address))
        .env("VEZKC_ADDRESS", format!("{:#x}", ctx.deployment.vezkc_address))
        .env("STAKING_REWARDS_ADDRESS", format!("{:#x}", ctx.deployment.staking_rewards_address))
        .env("RPC_URL", ctx.anvil.lock().await.endpoint_url().as_str())
        .env("PRIVATE_KEY", &user_private_key)
        .env("NO_COLOR", "1")
        .env("RUST_LOG", "boundless_cli=debug,info")
        .assert()
        .success()
        .stdout(contains(format_ether(stake_amount)));

    // Run get staked amount
    let mut cmd = Command::cargo_bin("boundless")?;
    cmd.args(["zkc", "get-staked-amount", &format!("{:#x}", user.address())])
        .env("ZKC_ADDRESS", format!("{:#x}", ctx.deployment.zkc_address))
        .env("VEZKC_ADDRESS", format!("{:#x}", ctx.deployment.vezkc_address))
        .env("STAKING_REWARDS_ADDRESS", format!("{:#x}", ctx.deployment.staking_rewards_address))
        .env("RPC_URL", ctx.anvil.lock().await.endpoint_url().as_str())
        .env("NO_COLOR", "1")
        .env("RUST_LOG", "boundless_cli=debug,info")
        .assert()
        .success()
        .stdout(contains(format_ether(amount)));

    // Run unstake
    let mut cmd = Command::cargo_bin("boundless")?;
    cmd.args(["zkc", "unstake"])
        .env("ZKC_ADDRESS", format!("{:#x}", ctx.deployment.zkc_address))
        .env("VEZKC_ADDRESS", format!("{:#x}", ctx.deployment.vezkc_address))
        .env("STAKING_REWARDS_ADDRESS", format!("{:#x}", ctx.deployment.staking_rewards_address))
        .env("RPC_URL", ctx.anvil.lock().await.endpoint_url().as_str())
        .env("PRIVATE_KEY", &user_private_key)
        .env("NO_COLOR", "1")
        .env("RUST_LOG", "boundless_cli=debug,info")
        .assert()
        .success()
        .stdout(contains("Unstaking completed"));

    // Run unstake again
    let mut cmd = Command::cargo_bin("boundless")?;
    cmd.args(["zkc", "unstake"])
        .env("ZKC_ADDRESS", format!("{:#x}", ctx.deployment.zkc_address))
        .env("VEZKC_ADDRESS", format!("{:#x}", ctx.deployment.vezkc_address))
        .env("STAKING_REWARDS_ADDRESS", format!("{:#x}", ctx.deployment.staking_rewards_address))
        .env("RPC_URL", ctx.anvil.lock().await.endpoint_url().as_str())
        .env("PRIVATE_KEY", &user_private_key)
        .env("NO_COLOR", "1")
        .env("RUST_LOG", "boundless_cli=debug,info")
        .assert()
        .failure()
        .stderr(contains("Unstaking initiated"));

    Ok(())
}

#[tokio::test]
async fn test_delegate_rewards() -> anyhow::Result<()> {
    // Set up a local Anvil node with the required contracts
    let ctx = test_ctx().await?;

    // Use an Anvil-provided signer for transaction signing (with balance)
    let user: PrivateKeySigner = ctx.anvil.lock().await.keys()[1].clone().into();
    let user_private_key = format!("0x{}", hex::encode(user.to_bytes()));
    let user2: PrivateKeySigner = ctx.anvil.lock().await.keys()[2].clone().into();

    // Fund the user
    let amount = U256::from(1_000_000_000);
    let stake_amount = U256::from(500_000_000);
    ctx.zkc.initialMint(vec![user.address()], vec![amount]).send().await?.watch().await?;

    // Run stake
    let mut cmd = Command::cargo_bin("boundless")?;
    cmd.args(["zkc", "stake", "--amount", stake_amount.to_string().as_str()])
        .env("ZKC_ADDRESS", format!("{:#x}", ctx.deployment.zkc_address))
        .env("VEZKC_ADDRESS", format!("{:#x}", ctx.deployment.vezkc_address))
        .env("STAKING_REWARDS_ADDRESS", format!("{:#x}", ctx.deployment.staking_rewards_address))
        .env("RPC_URL", ctx.anvil.lock().await.endpoint_url().as_str())
        .env("PRIVATE_KEY", &user_private_key)
        .env("NO_COLOR", "1")
        .env("RUST_LOG", "boundless_cli=debug,info")
        .assert()
        .success()
        .stdout(contains(format_ether(stake_amount)));

    // Run delegate rewards
    let mut cmd = Command::cargo_bin("boundless")?;
    cmd.args(["zkc", "delegate-rewards", &format!("{:#x}", user2.address())])
        .env("ZKC_ADDRESS", format!("{:#x}", ctx.deployment.zkc_address))
        .env("VEZKC_ADDRESS", format!("{:#x}", ctx.deployment.vezkc_address))
        .env("STAKING_REWARDS_ADDRESS", format!("{:#x}", ctx.deployment.staking_rewards_address))
        .env("RPC_URL", ctx.anvil.lock().await.endpoint_url().as_str())
        .env("PRIVATE_KEY", &user_private_key)
        .env("NO_COLOR", "1")
        .env("RUST_LOG", "boundless_cli=debug,info")
        .assert()
        .success()
        .stdout(contains("Delegating rewards completed"));

    // Run get rewards delegates
    let mut cmd = Command::cargo_bin("boundless")?;
    cmd.args(["zkc", "get-rewards-delegates", &format!("{:#x}", user.address())])
        .env("ZKC_ADDRESS", format!("{:#x}", ctx.deployment.zkc_address))
        .env("VEZKC_ADDRESS", format!("{:#x}", ctx.deployment.vezkc_address))
        .env("STAKING_REWARDS_ADDRESS", format!("{:#x}", ctx.deployment.staking_rewards_address))
        .env("RPC_URL", ctx.anvil.lock().await.endpoint_url().as_str())
        .env("NO_COLOR", "1")
        .env("RUST_LOG", "boundless_cli=debug,info")
        .assert()
        .success()
        .stdout(contains(format!("{:#x}", user2.address())));

    Ok(())
}

#[tokio::test]
async fn test_get_epoch_end_time() -> anyhow::Result<()> {
    // Set up a local Anvil node with the required contracts
    let ctx = test_ctx().await?;

    // Run get epoch 0 end time
    let mut cmd = Command::cargo_bin("boundless")?;
    cmd.args(["zkc", "get-epoch-end-time", "0"])
        .env("ZKC_ADDRESS", format!("{:#x}", ctx.deployment.zkc_address))
        .env("VEZKC_ADDRESS", format!("{:#x}", ctx.deployment.vezkc_address))
        .env("STAKING_REWARDS_ADDRESS", format!("{:#x}", ctx.deployment.staking_rewards_address))
        .env("RPC_URL", ctx.anvil.lock().await.endpoint_url().as_str())
        .env("NO_COLOR", "1")
        .env("RUST_LOG", "boundless_cli=debug,info")
        .assert()
        .success();

    Ok(())
}

#[tokio::test]
async fn test_get_current_epoch() -> anyhow::Result<()> {
    // Set up a local Anvil node with the required contracts
    let ctx = test_ctx().await?;

    // Run get current epoch
    let mut cmd = Command::cargo_bin("boundless")?;
    cmd.args(["zkc", "get-current-epoch"])
        .env("ZKC_ADDRESS", format!("{:#x}", ctx.deployment.zkc_address))
        .env("VEZKC_ADDRESS", format!("{:#x}", ctx.deployment.vezkc_address))
        .env("STAKING_REWARDS_ADDRESS", format!("{:#x}", ctx.deployment.staking_rewards_address))
        .env("RPC_URL", ctx.anvil.lock().await.endpoint_url().as_str())
        .env("NO_COLOR", "1")
        .env("RUST_LOG", "boundless_cli=debug,info")
        .assert()
        .success();

    Ok(())
}
