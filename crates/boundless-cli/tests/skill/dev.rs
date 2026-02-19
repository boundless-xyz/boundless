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

//! Integration tests for the skill dev command.

use predicates::str::contains;
use tempfile::TempDir;

use crate::skill::cli_cmd;

/// Test that the skill dev command shows help correctly.
#[test]
fn test_skill_dev_help() {
    let mut cmd = cli_cmd().unwrap();

    cmd.args(["skill", "dev", "--help"])
        .env("NO_COLOR", "1")
        .assert()
        .success()
        .stdout(contains("Usage:"))
        .stdout(contains("Symlink skill source files"))
        .stdout(contains("--global"))
        .stdout(contains("--path"))
        .stdout(contains("--clean"))
        .stderr("");
}

/// Test that skill dev with --path creates symlinks for all skills.
#[test]
fn test_skill_dev_creates_symlinks() {
    let tmp_dir = TempDir::new().unwrap();
    let target_path = tmp_dir.path().join("test-skills");

    let mut cmd = cli_cmd().unwrap();

    cmd.args(["skill", "dev", "--path", target_path.to_str().unwrap()])
        .env("NO_COLOR", "1")
        .assert()
        .success()
        .stdout(contains("Linked"))
        .stdout(contains("boundless-overview"))
        .stdout(contains("first-request"));

    // Verify symlinks were created.
    let overview_link = target_path.join("boundless-overview");
    let first_request_link = target_path.join("first-request");

    assert!(overview_link.symlink_metadata().unwrap().file_type().is_symlink());
    assert!(first_request_link.symlink_metadata().unwrap().file_type().is_symlink());

    // Verify the symlinks point to valid skill directories.
    assert!(overview_link.join("SKILL.md").exists());
    assert!(first_request_link.join("SKILL.md").exists());
}

/// Test that skill dev with a specific skill name only links that skill.
#[test]
fn test_skill_dev_single_skill() {
    let tmp_dir = TempDir::new().unwrap();
    let target_path = tmp_dir.path().join("test-single");

    let mut cmd = cli_cmd().unwrap();

    cmd.args(["skill", "dev", "boundless-overview", "--path", target_path.to_str().unwrap()])
        .env("NO_COLOR", "1")
        .assert()
        .success()
        .stdout(contains("Linked 'boundless-overview'"));

    // Only the requested skill should be linked.
    assert!(target_path
        .join("boundless-overview")
        .symlink_metadata()
        .unwrap()
        .file_type()
        .is_symlink());
    assert!(!target_path.join("first-request").exists());
}

/// Test that skill dev with --clean removes symlinks.
#[test]
fn test_skill_dev_clean() {
    let tmp_dir = TempDir::new().unwrap();
    let target_path = tmp_dir.path().join("test-clean");

    // First create symlinks.
    let mut cmd = cli_cmd().unwrap();
    cmd.args(["skill", "dev", "--path", target_path.to_str().unwrap()])
        .env("NO_COLOR", "1")
        .assert()
        .success();

    assert!(target_path.join("boundless-overview").exists());

    // Now clean them.
    let mut cmd = cli_cmd().unwrap();
    cmd.args(["skill", "dev", "--clean", "--path", target_path.to_str().unwrap()])
        .env("NO_COLOR", "1")
        .assert()
        .success()
        .stdout(contains("Removed symlink"))
        .stdout(contains("Cleaned"));

    assert!(!target_path.join("boundless-overview").exists());
    assert!(!target_path.join("first-request").exists());
}

/// Test that skill dev with an invalid name produces a helpful error.
#[test]
fn test_skill_dev_invalid_name() {
    let tmp_dir = TempDir::new().unwrap();
    let target_path = tmp_dir.path().join("test-invalid");

    let mut cmd = cli_cmd().unwrap();

    cmd.args(["skill", "dev", "nonexistent-skill", "--path", target_path.to_str().unwrap()])
        .env("NO_COLOR", "1")
        .assert()
        .failure()
        .stderr(contains("Unknown skill 'nonexistent-skill'"))
        .stderr(contains("boundless-overview"))
        .stderr(contains("first-request"));
}

/// Test that skill dev is idempotent (re-linking replaces existing symlinks).
#[test]
fn test_skill_dev_idempotent() {
    let tmp_dir = TempDir::new().unwrap();
    let target_path = tmp_dir.path().join("test-idem");

    // Link once.
    let mut cmd = cli_cmd().unwrap();
    cmd.args(["skill", "dev", "--path", target_path.to_str().unwrap()])
        .env("NO_COLOR", "1")
        .assert()
        .success();

    // Link again — should succeed without error.
    let mut cmd = cli_cmd().unwrap();
    cmd.args(["skill", "dev", "--path", target_path.to_str().unwrap()])
        .env("NO_COLOR", "1")
        .assert()
        .success()
        .stdout(contains("Linked"));

    assert!(target_path
        .join("boundless-overview")
        .symlink_metadata()
        .unwrap()
        .file_type()
        .is_symlink());
}

/// Test that skill dev refuses to overwrite a non-symlink directory.
#[test]
fn test_skill_dev_refuses_non_symlink() {
    let tmp_dir = TempDir::new().unwrap();
    let target_path = tmp_dir.path().join("test-refuse");

    // Create a real directory where the symlink would go.
    std::fs::create_dir_all(target_path.join("boundless-overview")).unwrap();
    std::fs::write(target_path.join("boundless-overview/SKILL.md"), "real file").unwrap();

    let mut cmd = cli_cmd().unwrap();

    cmd.args(["skill", "dev", "boundless-overview", "--path", target_path.to_str().unwrap()])
        .env("NO_COLOR", "1")
        .assert()
        .failure()
        .stderr(contains("not a symlink"));
}
