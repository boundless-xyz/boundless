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

use std::{path::Path, process::ExitStatus};

use anyhow::{anyhow, bail, Result};
use risc0_groth16::ProofJson as Groth16ProofJson;
use std::process::Command;

pub fn shrink_wrap(
    work_dir: &Path,
    identity_seal_json: serde_json::Value,
) -> Result<Groth16ProofJson> {
    tracing::debug!("blake3 shrink_wrap with docker rapidsnark");
    if !is_docker_installed() {
        bail!("Please install docker first.")
    }

    let proof_path = work_dir.join("proof.json");
    let input_path = work_dir.join("input.json");
    std::fs::write(&input_path, identity_seal_json.to_string())?;

    let output = Command::new("docker")
        .arg("run")
        .arg("--rm")
        .arg("-v")
        .arg(format!("{}:/mnt", work_dir.display()))
        .arg("boundless-blake3-g16:latest")
        .output()?;
    if !output.status.success() {
        return Err(error_from_status(output.status));
    }

    let proof_content = std::fs::read_to_string(&proof_path)?;

    let proof_json: Groth16ProofJson =
        serde_json::from_str(proof_content.trim_matches(char::from(0)))?;

    Ok(proof_json)
}

fn is_docker_installed() -> bool {
    Command::new("docker")
        .arg("--version")
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false)
}

fn error_from_status(status: ExitStatus) -> anyhow::Error {
    let err = anyhow!("docker returned failure exit code: {:?}", status.code());
    // Add a hint to the error context for certain error codes.
    match status.code() {
        Some(126) => err.context("This process may not have permission to run containers."),
        _ => err,
    }
}
