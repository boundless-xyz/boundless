// Copyright 2025 Boundless Foundation, Inc.
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

use anyhow::{anyhow, Result};
use std::path::Path;

pub fn calculate_witness_encoded(graph_path: &Path, inputs: &str) -> Result<Vec<u8>> {
    tracing::info!("calculate_witness");
    let graph = std::fs::read(graph_path)?;
    let witness_encoded = circom_witnesscalc::calc_witness(inputs, &graph)
        .map_err(|err| anyhow!("witness failure: {err}"))?;
    Ok(witness_encoded)
}
