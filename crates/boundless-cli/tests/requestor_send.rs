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
async fn test_send_help() {
    common::BoundlessCmd::new("requestor", "send")
        .arg("--help")
        .assert()
        .success()
        .stdout(contains("Usage:"))
        .stdout(contains("send"));
}

#[tokio::test]
async fn test_send_without_recipient() {
    let ctx = TestContext::base().await;
    let account = ctx.account(0);

    // Should fail without recipient
    ctx.cmd("requestor", "send")
        .arg("--amount")
        .arg("0.01")
        .with_account(&account)
        .assert()
        .failure();
}

#[tokio::test]
async fn test_send_dry_run() {
    let ctx = TestContext::base().await;
    let sender = ctx.account(0);
    let recipient = ctx.account(1);

    // Dry run should succeed without actually sending
    ctx.cmd("requestor", "send")
        .arg("--to")
        .arg(&recipient.address)
        .arg("--amount")
        .arg("0.01")
        .arg("--dry-run")
        .with_account(&sender)
        .assert()
        .success()
        .stdout(contains("Dry run"));
}
