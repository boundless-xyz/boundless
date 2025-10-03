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
async fn test_list_orders_help() {
    common::BoundlessCmd::new("requestor", "list-orders")
        .arg("--help")
        .assert()
        .success()
        .stdout(contains("Usage:"))
        .stdout(contains("list-orders"));
}

#[tokio::test]
async fn test_list_orders_with_address() {
    let ctx = TestContext::base().await;
    let account = ctx.account(0);

    // Should list orders for the given address
    ctx.cmd("requestor", "list-orders").arg(&account.address).assert().success();
}

#[tokio::test]
async fn test_list_orders_with_limit() {
    let ctx = TestContext::base().await;
    let account = ctx.account(0);

    // Should respect limit parameter
    ctx.cmd("requestor", "list-orders")
        .arg(&account.address)
        .arg("--limit")
        .arg("5")
        .assert()
        .success();
}
