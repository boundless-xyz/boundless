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
async fn test_staked_balance_zkc_with_address() {
    let ctx = TestContext::ethereum().await;
    let account = ctx.account(0);

    ctx.cmd("rewards", "staked-balance-zkc")
        .arg(&account.address)
        .assert()
        .success()
        .stdout(contains("Staked ZKC"));
}

#[tokio::test]
async fn test_staked_balance_zkc_zero_address() {
    let ctx = TestContext::ethereum().await;

    // Zero address might fail for NFT contracts
    ctx.cmd("rewards", "staked-balance-zkc")
        .arg("0x0000000000000000000000000000000000000000")
        .assert()
        .failure();
}

#[tokio::test]
async fn test_staked_balance_zkc_help() {
    // No context needed for help
    common::BoundlessCmd::new("rewards", "staked-balance-zkc")
        .arg("--help")
        .assert()
        .success()
        .stdout(contains("Usage:"))
        .stdout(contains("staked-balance-zkc"));
}
