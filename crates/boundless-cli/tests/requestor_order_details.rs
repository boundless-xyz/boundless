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
async fn test_order_details_help() {
    common::BoundlessCmd::new("requestor", "order-details")
        .arg("--help")
        .assert()
        .success()
        .stdout(contains("Usage:"))
        .stdout(contains("order-details"));
}

#[tokio::test]
async fn test_order_details_invalid_id() {
    let ctx = TestContext::base().await;

    // Should handle invalid order ID gracefully
    ctx.cmd("requestor", "order-details")
        .arg("0x0000000000000000000000000000000000000000000000000000000000000000")
        .assert()
        .success();  // Command should succeed even if order doesn't exist
}