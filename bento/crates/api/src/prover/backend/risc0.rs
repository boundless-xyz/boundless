// Copyright 2026 Boundless Foundation, Inc.
//
// Use of this source code is governed by the Business Source License
// as found in the LICENSE-BSL file.

use super::ExecutionJob;
use anyhow::{Context, Result};
use risc0_zkvm::{ExecutorEnv, Prover, VerifierContext, default_prover};

pub fn prove(job: &ExecutionJob) -> Result<Vec<u8>> {
    let program_bytes = std::fs::read(&job.program_path).with_context(|| {
        format!("Failed to read risc0 program ELF at {}", job.program_path.display())
    })?;
    let input_bytes = std::fs::read(&job.input_path).with_context(|| {
        format!("Failed to read risc0 prover input at {}", job.input_path.display())
    })?;

    let env = ExecutorEnv::builder()
        .write_slice(&input_bytes)
        .build()
        .context("Failed to build risc0 executor environment")?;

    let prove_info = default_prover()
        .prove(env, &program_bytes)
        .context("risc0 default_prover prove() failed")?;

    if job.verify {
        prove_info
            .receipt
            .verify_integrity_with_context(&VerifierContext::default())
            .context("risc0 receipt integrity verification failed")?;
    }

    bincode::serialize(&prove_info.receipt).context("Failed to serialize risc0 receipt")
}
