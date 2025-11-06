use std::path::Path;

use crate::prove::witgen::calculate_witness_encoded;

use anyhow::Result;
use risc0_groth16::ProofJson as Groth16ProofJson;
use std::process::Command;

pub fn shrink_wrap(
    work_dir: &Path,
    identity_seal_json: serde_json::Value,
) -> Result<Groth16ProofJson> {
    tracing::debug!("shrink_wrap with rapidsnark");
    let root_dir = std::env::var("BLAKE3_GROTH16_SETUP_DIR");
    let root_dir = root_dir
        .as_ref()
        .map(Path::new)
        .expect("must provide BLAKE3_GROTH16_SETUP_DIR");

    let graph_path = root_dir.join("verify_for_guest_graph.bin");
    let witness_path = work_dir.join("output.wtns");
    let proof_path = work_dir.join("proof.json");
    let public_path = work_dir.join("public.json");

    let witness_encoded =
        calculate_witness_encoded(&graph_path, identity_seal_json.to_string().as_str())?;
    std::fs::write(&witness_path, witness_encoded)?;

    let status = Command::new("rapidsnark")
        .arg(root_dir.join("verify_for_guest_final.zkey"))
        .arg(witness_path)
        .arg(&proof_path)
        .arg(public_path)
        .status()?;

    anyhow::ensure!(status.success(), "rapidsnark failed: {:?}", status.code());

    let proof_content = std::fs::read_to_string(&proof_path)?;

    let proof_json: Groth16ProofJson =
        serde_json::from_str(proof_content.trim_matches(char::from(0)))?;

    Ok(proof_json)
}
