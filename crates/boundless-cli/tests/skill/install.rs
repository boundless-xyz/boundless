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

//! Integration tests for the skill install command.

use predicates::str::contains;
use tempfile::TempDir;

use crate::skill::cli_cmd;

/// Test that the skill install command shows help correctly.
#[test]
fn test_skill_install_help() {
    let mut cmd = cli_cmd().unwrap();

    cmd.args(["skill", "install", "--help"])
        .env("NO_COLOR", "1")
        .assert()
        .success()
        .stdout(contains("Usage:"))
        .stdout(contains("Install Boundless skill files"))
        .stdout(contains("--global"))
        .stdout(contains("--path"))
        .stderr("");
}

/// Test that the skill subcommand shows help correctly.
#[test]
fn test_skill_help() {
    let mut cmd = cli_cmd().unwrap();

    cmd.args(["skill", "--help"])
        .env("NO_COLOR", "1")
        .assert()
        .success()
        .stdout(contains("Usage:"))
        .stdout(contains("install"))
        .stderr("");
}

/// Test that skill install with --path creates files at the specified location.
#[test]
fn test_skill_install_with_path() {
    let tmp_dir = TempDir::new().unwrap();
    let install_path = tmp_dir.path().join("test-skill");

    let mut cmd = cli_cmd().unwrap();

    cmd.args(["skill", "install", "--path", install_path.to_str().unwrap()])
        .env("NO_COLOR", "1")
        .assert()
        .success()
        .stdout(contains("Boundless skill installed successfully"))
        .stdout(contains("SKILL.md"))
        .stdout(contains("references/architecture.md"))
        .stdout(contains("references/cli-reference.md"))
        .stdout(contains("references/conventions.md"));

    // Verify files were created
    assert!(install_path.join("SKILL.md").exists());
    assert!(install_path.join("references/architecture.md").exists());
    assert!(install_path.join("references/cli-reference.md").exists());
    assert!(install_path.join("references/conventions.md").exists());

    // Verify SKILL.md contains expected content
    let skill_content = std::fs::read_to_string(install_path.join("SKILL.md")).unwrap();
    assert!(skill_content.contains("Boundless CLI Skill"));
    assert!(skill_content.contains("boundless requestor"));
    assert!(skill_content.contains("boundless prover"));
    assert!(skill_content.contains("boundless rewards"));
}

/// Test that running skill install with --path twice overwrites cleanly.
#[test]
fn test_skill_install_idempotent() {
    let tmp_dir = TempDir::new().unwrap();
    let install_path = tmp_dir.path().join("test-skill");

    // Install once
    let mut cmd = cli_cmd().unwrap();
    cmd.args(["skill", "install", "--path", install_path.to_str().unwrap()])
        .env("NO_COLOR", "1")
        .assert()
        .success();

    // Install again - should succeed without error
    let mut cmd = cli_cmd().unwrap();
    cmd.args(["skill", "install", "--path", install_path.to_str().unwrap()])
        .env("NO_COLOR", "1")
        .assert()
        .success()
        .stdout(contains("Boundless skill installed successfully"));

    assert!(install_path.join("SKILL.md").exists());
}
