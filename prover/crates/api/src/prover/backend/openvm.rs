// Copyright 2026 Boundless Foundation, Inc.
//
// Use of this source code is governed by the Business Source License
// as found in the LICENSE-BSL file.

use super::{ExecutionJob, run_cli_backend};
use anyhow::Result;

pub fn prove(job: &ExecutionJob) -> Result<Vec<u8>> {
    run_cli_backend(job, "OPENVM_PROVER_BIN", "openvm-prover")
}
