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
        .stdout(contains("--all"))
        .stdout(contains("--list"))
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

/// Test that skill install with --path and skill name creates overview files.
#[test]
fn test_skill_install_with_path() {
    let tmp_dir = TempDir::new().unwrap();
    let install_path = tmp_dir.path().join("test-skill");

    let mut cmd = cli_cmd().unwrap();

    cmd.args([
        "skill",
        "install",
        "boundless-overview",
        "--path",
        install_path.to_str().unwrap(),
    ])
    .env("NO_COLOR", "1")
    .assert()
    .success()
    .stdout(contains("installed successfully"))
    .stdout(contains("SKILL.md"))
    .stdout(contains("references/architecture.md"))
    .stdout(contains("references/cli-reference.md"))
    .stdout(contains("references/conventions.md"));

    // Verify files were created under the skill name subdirectory.
    let skill_dir = install_path.join("boundless-overview");
    assert!(skill_dir.join("SKILL.md").exists());
    assert!(skill_dir.join("references/architecture.md").exists());
    assert!(skill_dir.join("references/cli-reference.md").exists());
    assert!(skill_dir.join("references/conventions.md").exists());

    // Verify SKILL.md contains expected content.
    let skill_content = std::fs::read_to_string(skill_dir.join("SKILL.md")).unwrap();
    assert!(skill_content.contains("Boundless CLI Skill"));
    assert!(skill_content.contains("boundless requestor"));
    assert!(skill_content.contains("boundless prover"));
    assert!(skill_content.contains("boundless rewards"));
}

/// Test that echo-request skill installs all 7 files correctly.
#[test]
fn test_skill_install_echo_request_with_path() {
    let tmp_dir = TempDir::new().unwrap();
    let install_path = tmp_dir.path().join("test-echo");

    let mut cmd = cli_cmd().unwrap();

    cmd.args([
        "skill",
        "install",
        "boundless-echo-request",
        "--path",
        install_path.to_str().unwrap(),
    ])
    .env("NO_COLOR", "1")
    .assert()
    .success()
    .stdout(contains("installed successfully"));

    // Verify all 7 files were created.
    let skill_dir = install_path.join("boundless-echo-request");
    assert!(skill_dir.join("SKILL.md").exists());
    assert!(skill_dir.join("scripts/check-prerequisites.sh").exists());
    assert!(skill_dir.join("scripts/discover-programs.sh").exists());
    assert!(skill_dir.join("examples/echo-request.yaml").exists());
    assert!(skill_dir.join("references/cli-reference.md").exists());
    assert!(skill_dir.join("references/echo-explainer.md").exists());
    assert!(skill_dir.join("references/troubleshooting.md").exists());

    // Verify SKILL.md contains expected echo-request content.
    let skill_content = std::fs::read_to_string(skill_dir.join("SKILL.md")).unwrap();
    assert!(skill_content.contains("echo"));
}

/// Test that --all installs both skills.
#[test]
fn test_skill_install_all_with_path() {
    let tmp_dir = TempDir::new().unwrap();
    let install_path = tmp_dir.path().join("test-all");

    let mut cmd = cli_cmd().unwrap();

    cmd.args([
        "skill",
        "install",
        "--all",
        "--path",
        install_path.to_str().unwrap(),
    ])
    .env("NO_COLOR", "1")
    .assert()
    .success()
    .stdout(contains("installed successfully"));

    // Verify both skill directories exist with their files.
    assert!(install_path.join("boundless-overview/SKILL.md").exists());
    assert!(
        install_path
            .join("boundless-overview/references/architecture.md")
            .exists()
    );
    assert!(
        install_path
            .join("boundless-echo-request/SKILL.md")
            .exists()
    );
    assert!(
        install_path
            .join("boundless-echo-request/scripts/check-prerequisites.sh")
            .exists()
    );
    assert!(
        install_path
            .join("boundless-echo-request/examples/echo-request.yaml")
            .exists()
    );
}

/// Test that --list prints available skills without installing.
#[test]
fn test_skill_install_list() {
    let mut cmd = cli_cmd().unwrap();

    cmd.args(["skill", "install", "--list"])
        .env("NO_COLOR", "1")
        .assert()
        .success()
        .stdout(contains("boundless-overview"))
        .stdout(contains("boundless-echo-request"))
        .stdout(contains("Boundless CLI overview"))
        .stdout(contains("Guided walkthrough"));
}

/// Test that an invalid skill name produces a helpful error.
#[test]
fn test_skill_install_invalid_name() {
    let tmp_dir = TempDir::new().unwrap();
    let install_path = tmp_dir.path().join("test-invalid");

    let mut cmd = cli_cmd().unwrap();

    cmd.args([
        "skill",
        "install",
        "nonexistent-skill",
        "--path",
        install_path.to_str().unwrap(),
    ])
    .env("NO_COLOR", "1")
    .assert()
    .failure()
    .stderr(contains("Unknown skill 'nonexistent-skill'"))
    .stderr(contains("boundless-overview"))
    .stderr(contains("boundless-echo-request"));
}

/// Test that running skill install twice overwrites cleanly.
#[test]
fn test_skill_install_idempotent() {
    let tmp_dir = TempDir::new().unwrap();
    let install_path = tmp_dir.path().join("test-skill");

    // Install once.
    let mut cmd = cli_cmd().unwrap();
    cmd.args([
        "skill",
        "install",
        "boundless-overview",
        "--path",
        install_path.to_str().unwrap(),
    ])
    .env("NO_COLOR", "1")
    .assert()
    .success();

    // Install again - should succeed without error.
    let mut cmd = cli_cmd().unwrap();
    cmd.args([
        "skill",
        "install",
        "boundless-overview",
        "--path",
        install_path.to_str().unwrap(),
    ])
    .env("NO_COLOR", "1")
    .assert()
    .success()
    .stdout(contains("installed successfully"));

    assert!(
        install_path
            .join("boundless-overview/SKILL.md")
            .exists()
    );
}
