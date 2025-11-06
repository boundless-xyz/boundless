use anyhow::{anyhow, Result};
use std::path::Path;

pub fn calculate_witness_encoded(graph_path: &Path, inputs: &str) -> Result<Vec<u8>> {
    tracing::info!("calculate_witness");
    let graph = std::fs::read(graph_path)?;
    let witness_encoded = circom_witnesscalc::calc_witness(inputs, &graph)
        .map_err(|err| anyhow!("witness failure: {err}"))?;
    Ok(witness_encoded)
}
