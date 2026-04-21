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

use std::{collections::BTreeMap, fmt, sync::Arc};

use serde::{Deserialize, Serialize};

/// Supported zkVM backends. Serialized as lowercase strings on the wire
/// (e.g. `"risc0"`).
///
/// Add new backends here, then register an [`Executor`] implementation in
/// [`crate::backends::default_registry`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ZkvmKind {
    Risc0,
}

impl fmt::Display for ZkvmKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            ZkvmKind::Risc0 => "risc0",
        })
    }
}

/// Result of a successful execution. All cycle counts are best-effort: some
/// backends report only a single aggregate value, in which case `user_cycles`
/// mirrors `total_cycles`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionStats {
    pub zkvm: ZkvmKind,
    /// Total cycles executed (including overhead / padding when applicable).
    pub total_cycles: u64,
    /// User-program cycles (instructions retired in the guest). Falls back to
    /// `total_cycles` when the backend does not distinguish the two.
    pub user_cycles: u64,
    /// Journal / public output bytes produced by the guest, hex-encoded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub journal_hex: Option<String>,
    /// Wall-clock time spent inside the executor, in milliseconds.
    pub elapsed_ms: u64,
}

#[derive(Debug, thiserror::Error)]
pub enum ExecutorError {
    #[error("zkvm backend `{0}` is not enabled in this build")]
    BackendDisabled(ZkvmKind),
    #[error("invalid elf: {0}")]
    InvalidElf(String),
    #[error("invalid input: {0}")]
    InvalidInput(String),
    #[error("execution failed: {0}")]
    Execution(#[source] anyhow::Error),
}

/// A single zkVM execution backend.
///
/// Implementations run the provided ELF against the provided raw input bytes
/// and return summary statistics. They must be safe to call from multiple
/// tokio tasks concurrently.
pub trait Executor: Send + Sync + 'static {
    fn kind(&self) -> ZkvmKind;

    fn execute(&self, elf: &[u8], input: &[u8]) -> Result<ExecutionStats, ExecutorError>;
}

/// Registry mapping a [`ZkvmKind`] to its configured backend.
#[derive(Clone, Default)]
pub struct Registry {
    inner: BTreeMap<ZkvmKind, Arc<dyn Executor>>,
}

impl Registry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with(mut self, exec: Arc<dyn Executor>) -> Self {
        self.inner.insert(exec.kind(), exec);
        self
    }

    pub fn kinds(&self) -> Vec<ZkvmKind> {
        self.inner.keys().copied().collect()
    }

    pub fn get(&self, kind: ZkvmKind) -> Option<Arc<dyn Executor>> {
        self.inner.get(&kind).cloned()
    }
}
