use std::{path::Path, process::ExitStatus};

use crate::prove::witgen::calculate_witness_encoded;

use anyhow::{anyhow, bail, Result};
use risc0_groth16::ProofJson as Groth16ProofJson;
use std::process::Command;

pub fn shrink_wrap(
    work_dir: &Path,
    identity_seal_json: serde_json::Value,
) -> Result<Groth16ProofJson> {
    tracing::debug!("shrink_wrap with rapidsnark");
    if !is_docker_installed() {
        bail!("Please install docker first.")
    }
    let root_dir = std::env::var("BLAKE3_GROTH16_SETUP_DIR");
    let root_dir = root_dir.as_ref().map(Path::new).expect("must provide BLAKE3_GROTH16_SETUP_DIR");

    let graph_path = root_dir.join("verify_for_guest_graph.bin");
    let witness_path = work_dir.join("output.wtns");
    let proof_path = work_dir.join("proof.json");
    let public_path = work_dir.join("public.json");

    let witness_encoded =
        calculate_witness_encoded(&graph_path, identity_seal_json.to_string().as_str())?;
    std::fs::write(&witness_path, witness_encoded)?;

    let output = Command::new("docker")
        .arg("run")
        .arg("--rm")
        .arg("-v")
        .arg(format!("{}:/mnt", work_dir.display()))
        .arg("-v")
        .arg(format!("{}:/setup", root_dir.display()))
        .arg("rpsnark:latest")
        .arg("/setup/verify_for_guest_final.zkey")
        .arg(format!("/mnt/{}", witness_path.file_name().unwrap().to_str().unwrap()))
        .arg(format!("/mnt/{}", proof_path.file_name().unwrap().to_str().unwrap()))
        .arg(format!("/mnt/{}", public_path.file_name().unwrap().to_str().unwrap()))
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
