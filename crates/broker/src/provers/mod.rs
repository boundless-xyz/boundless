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

use boundless_market::input::GuestEnv;

mod bonsai;
mod default;

pub use bonsai::Bonsai;
pub use default::DefaultProver;

// Re-export types from boundless_market::prover_utils::prover
pub use boundless_market::prover_utils::prover::{
    ExecutorResp, ProofResult, Prover, ProverError, ProverObj,
};

/// Encode inputs for Prover::upload_slice()
pub fn encode_input(input: &impl serde::Serialize) -> Result<Vec<u8>, anyhow::Error> {
    Ok(GuestEnv::builder().write(input)?.stdin)
}
