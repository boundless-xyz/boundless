//! Integration tests for PoVW-related CLI commands.

use assert_cmd::Command;

/// Test that the PoVW prove-update command shows help correctly.
/// This is a smoke test to ensure the command is properly registered and accessible.
#[test]
fn test_prove_update_help() {
    let mut cmd = Command::cargo_bin("boundless").unwrap();

    cmd.args(["povw", "prove-update", "--help"])
        .assert()
        .success()
        .stdout(predicates::str::contains("Usage:"))
        .stdout(predicates::str::contains("prove-update"))
        .stderr("");
}
