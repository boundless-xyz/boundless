// Copyright 2026 Boundless Foundation, Inc.
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

use assert_cmd::Command;
use predicates::prelude::*;

#[test]
fn test_help_command() {
    let mut cmd = Command::cargo_bin("boundless").unwrap();
    cmd.arg("--help").assert().success().stdout(predicate::str::contains("Commands:"));
}

#[test]
fn test_requestor_commands_available() {
    let mut cmd = Command::cargo_bin("boundless").unwrap();
    cmd.arg("requestor")
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("balance"))
        .stdout(predicate::str::contains("deposit"))
        .stdout(predicate::str::contains("withdraw"));
}

#[test]
fn test_prover_commands_available() {
    let mut cmd = Command::cargo_bin("boundless").unwrap();
    cmd.arg("prover")
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("balance-collateral"))
        .stdout(predicate::str::contains("deposit-collateral"))
        .stdout(predicate::str::contains("withdraw-collateral"));
}

#[test]
fn test_rewards_commands_available() {
    let mut cmd = Command::cargo_bin("boundless").unwrap();
    cmd.arg("rewards")
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("balance-zkc"))
        .stdout(predicate::str::contains("stake-zkc"))
        .stdout(predicate::str::contains("staked-balance-zkc"))
        .stdout(predicate::str::contains("claim-staking-rewards"));
}

#[test]
fn test_setup_commands_available() {
    let mut cmd = Command::cargo_bin("boundless").unwrap();
    cmd.arg("requestor")
        .arg("setup")
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("network"));
}

#[test]
fn test_version_command() {
    let mut cmd = Command::cargo_bin("boundless").unwrap();
    cmd.arg("--version").assert().success().stdout(predicate::str::contains("boundless"));
}
