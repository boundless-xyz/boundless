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

mod common;
use common::TestContext;
use predicates::str::contains;

#[tokio::test]
async fn test_balance_collateral_with_address() {
    let ctx = TestContext::base().await;
    let account = ctx.account(0);

    ctx.cmd("prover", "balance-collateral")
        .arg(&account.address)
        .assert()
        .success()
        .stdout(contains("Collateral balance"));
}

#[tokio::test]
async fn test_balance_collateral_zero_address_fails() {
    let ctx = TestContext::base().await;

    ctx.cmd("prover", "balance-collateral")
        .arg("0x0000000000000000000000000000000000000000")
        .assert()
        .failure()
        .stderr(contains("No address specified"));
}

#[tokio::test]
async fn test_balance_collateral_with_private_key() {
    let ctx = TestContext::base().await;
    let account = ctx.account(1);

    // Test with both address and private key provided
    ctx.cmd("prover", "balance-collateral")
        .arg(&account.address)
        .with_account(&account)
        .assert()
        .success()
        .stdout(contains("Collateral balance"));
}

#[tokio::test]
async fn test_balance_collateral_help() {
    // No context needed for help
    common::BoundlessCmd::new("prover", "balance-collateral")
        .arg("--help")
        .assert()
        .success()
        .stdout(contains("Usage:"))
        .stdout(contains("balance-collateral"));
}
