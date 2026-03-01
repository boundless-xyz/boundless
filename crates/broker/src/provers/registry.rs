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

use alloy::primitives::FixedBytes;
use boundless_market::selector::{ProofType, SupportedSelectors};

use super::ProverObj;

/// Priority level for prover selection.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProverPriority {
    Standard,
    High,
}

/// Prover config for a single proof type, with optional high-priority override.
#[derive(Clone)]
pub(crate) struct ProverEntry {
    pub(crate) standard: ProverObj,
    /// Falls back to standard if None.
    pub(crate) high_priority: Option<ProverObj>,
}

impl ProverEntry {
    fn get(&self, priority: ProverPriority) -> &ProverObj {
        match priority {
            ProverPriority::Standard => &self.standard,
            ProverPriority::High => self.high_priority.as_ref().unwrap_or(&self.standard),
        }
    }
}

/// Registry mapping proof types and priorities to prover implementations.
#[derive(Clone)]
pub(crate) struct ProverRegistry {
    /// Default entry — ProofType::Any, Inclusion, and any unrecognized types.
    /// Also used for Groth16 compression since it shares the same backend.
    pub(crate) risc0_default: ProverEntry,
    /// Blake3Groth16 proof type. Currently shares the default entry, but will
    /// be given a distinct prover in the future.
    pub(crate) blake3_groth16: ProverEntry,
}

impl ProverRegistry {
    /// Convenience constructor for tests — all proof types share the same entry.
    #[cfg(test)]
    pub(crate) fn new_test(standard: ProverObj) -> Self {
        let entry = ProverEntry { standard, high_priority: None };
        Self { risc0_default: entry.clone(), blake3_groth16: entry }
    }

    /// Get prover by proof type and priority.
    pub(crate) fn get(&self, proof_type: ProofType, priority: ProverPriority) -> &ProverObj {
        match proof_type {
            ProofType::Blake3Groth16 => self.blake3_groth16.get(priority),
            _ => self.risc0_default.get(priority),
        }
    }

    /// Get prover by selector, resolving to proof type first.
    ///
    /// Returns `None` only if the selector is not in `supported`.
    #[allow(dead_code)] // Will be used by selector-aware proving in future work
    pub(crate) fn get_by_selector(
        &self,
        selector: FixedBytes<4>,
        supported: &SupportedSelectors,
        priority: ProverPriority,
    ) -> Option<&ProverObj> {
        let proof_type = supported.proof_type(selector)?;
        Some(self.get(proof_type, priority))
    }

    /// Standard default prover for STARK proving, uploads, and cancellation.
    pub(crate) fn standard(&self) -> &ProverObj {
        self.risc0_default.get(ProverPriority::Standard)
    }

    /// High-priority default prover for aggregation (bypasses type routing).
    pub(crate) fn high_priority(&self) -> &ProverObj {
        self.risc0_default.get(ProverPriority::High)
    }
}
