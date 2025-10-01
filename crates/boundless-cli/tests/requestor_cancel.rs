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
async fn test_cancel_help() {
    common::BoundlessCmd::new("requestor", "cancel")
        .arg("--help")
        .assert()
        .success()
        .stdout(contains("Usage:"))
        .stdout(contains("cancel"));
}

#[tokio::test]
async fn test_cancel_dry_run() {
    let ctx = TestContext::base().await;
    let account = ctx.account(0);

    // Dry run with a dummy order ID
    ctx.cmd("requestor", "cancel")
        .arg("0x0000000000000000000000000000000000000000000000000000000000000001")
        .arg("--dry-run")
        .with_account(&account)
        .assert()
        .success()
        .stdout(contains("Dry run"));
}