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

use std::{path::PathBuf, time::Instant};

use anyhow::{anyhow, Context};
use risc0_zkvm::{Executor as _, ExecutorEnv, ExternalProver};

use crate::backend::{ExecutionStats, Executor, ExecutorError, ZkvmKind};

pub struct Risc0Executor {
    inner: ExternalProver,
}

impl Risc0Executor {
    /// Construct a new executor, locating the `r0vm` binary via the
    /// `RISC0_SERVER_PATH` env var or `PATH`.
    pub fn new() -> anyhow::Result<Self> {
        let r0vm_path = locate_r0vm()?;
        tracing::debug!(r0vm = %r0vm_path.display(), "risc0 executor ready");
        Ok(Self { inner: ExternalProver::new("ipc", r0vm_path) })
    }
}

impl Executor for Risc0Executor {
    fn kind(&self) -> ZkvmKind {
        ZkvmKind::Risc0
    }

    fn execute(&self, elf: &[u8], input: &[u8]) -> Result<ExecutionStats, ExecutorError> {
        let env = ExecutorEnv::builder()
            .write_slice(input)
            .build()
            .map_err(|e| ExecutorError::InvalidInput(e.to_string()))?;

        let start = Instant::now();
        let info =
            self.inner.execute(env, elf).map_err(|e| ExecutorError::Execution(anyhow!(e)))?;
        let elapsed = start.elapsed();

        let user_cycles: u64 = info.segments.iter().map(|s| s.cycles as u64).sum();
        let total_cycles: u64 = info.segments.iter().map(|s| 1u64 << s.po2).sum();
        let journal_bytes = &info.journal.bytes;
        let journal_hex =
            if journal_bytes.is_empty() { None } else { Some(hex::encode(journal_bytes)) };

        Ok(ExecutionStats {
            zkvm: ZkvmKind::Risc0,
            total_cycles,
            user_cycles,
            journal_hex,
            elapsed_ms: elapsed.as_millis() as u64,
        })
    }
}

fn locate_r0vm() -> anyhow::Result<PathBuf> {
    if let Ok(p) = std::env::var("RISC0_SERVER_PATH") {
        let path = PathBuf::from(p);
        if path.is_file() {
            return Ok(path);
        }
    }
    if let Some(paths) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&paths) {
            let candidate = dir.join("r0vm");
            if candidate.is_file() {
                return Ok(candidate);
            }
        }
    }
    Err(anyhow!("r0vm binary not found; install with `rzup install r0vm` or set RISC0_SERVER_PATH"))
        .context("locating r0vm")
}
