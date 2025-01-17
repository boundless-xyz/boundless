// Copyright (c) 2024 RISC Zero, Inc.
//
// All rights reserved.

use risc0_zkvm::sha::Digest;
use serde::{Deserialize, Serialize};

pub mod s3;

/// Aux worker stream identifier
pub const AUX_WORK_TYPE: &str = "aux";

/// Executor worker stream identifier
pub const EXEC_WORK_TYPE: &str = "exec";

/// prove+lift worker stream identifier
pub const PROVE_WORK_TYPE: &str = "prove";

/// keccak/coproc worker stream identifier
pub const COPROC_WORK_TYPE: &str = "coproc";

/// join worker stream identifier
pub const JOIN_WORK_TYPE: &str = "join";

/// SNARK worker stream identifier
pub const SNARK_WORK_TYPE: &str = "snark";

pub const SNARK_RETRIES: i32 = 0;
pub const SNARK_TIMEOUT: i32 = 60 * 2;

#[derive(Deserialize, Serialize, Clone, Copy, PartialEq)]
pub enum CompressType {
    None,
    Groth16,
}

impl Default for CompressType {
    fn default() -> Self {
        Self::None
    }
}

/// Executor / init request
#[derive(Deserialize, Serialize)]
pub struct ExecutorReq {
    /// Image uuid
    pub image: String,
    /// Input uuid
    pub input: String,
    /// user identifier to pick the work stream
    pub user_id: String,
    /// Array of assumptions to include, urls
    pub assumptions: Vec<String>,
    /// toggles this task to only run the executor
    pub execute_only: bool,
    /// toggles automatic compression via selected compressor
    ///
    /// Defaults to None
    pub compress: CompressType,
    /// Execution limits (in mcycles)
    pub exec_limit: Option<u64>,
}

/// Executor output
#[derive(Clone, Deserialize, Serialize)]
pub struct ExecutorResp {
    /// Total segments output
    pub segments: u64,
    /// risc0-zkvm user cycles
    pub user_cycles: u64,
    /// risc0-zkvm total cycles
    pub total_cycles: u64,
    /// Count of assumptions included
    pub assumption_count: u64,
}

/// prove + lift task definition
#[derive(Deserialize, Serialize)]
pub struct ProveReq {
    /// Segment index
    pub index: usize,
}

/// Join Request
#[derive(Deserialize, Serialize, Clone)]
pub struct JoinReq {
    /// Node index
    pub idx: usize,
    /// index of the left branch
    pub left: usize,
    /// index of the right branch
    pub right: usize,
}

/// Resolve Request
#[derive(Deserialize, Serialize, Clone)]
pub struct ResolveReq {
    /// Index of the final joined receipt
    pub max_idx: usize,
}

/// Input request
#[derive(Deserialize, Serialize, Debug)]
pub struct FinalizeReq {
    /// Index of the final joined receipt
    pub max_idx: usize,
}

/// Snark task definition
#[derive(Serialize, Deserialize)]
pub struct SnarkReq {
    /// Stark receipt UUID to pull from minio
    pub receipt: String,
    /// Type of snark compression to run
    pub compress_type: CompressType,
}

/// Snark task response
#[derive(Serialize, Deserialize)]
pub struct SnarkResp {
    /// Snark UUID in object store snarks/{snark}
    pub snark: String,
}

/// Keccak prove request
#[derive(Serialize, Deserialize)]
pub struct KeccakReq {
    /// The digest of the claim that this keccak input is expected to produce.
    pub claim_digest: Digest,
    /// The requested size of the keccak proof, in powers of 2.
    pub po2: usize,
    /// The control root which identifies a particular keccak circuit revision.
    pub control_root: Digest,
}

/// High level enum of different sub task types and data
#[derive(Deserialize, Serialize)]
pub enum TaskType {
    /// Executor task
    Executor(ExecutorReq),
    /// rv32im Prove + lift task
    Prove(ProveReq),
    /// Join task
    Join(JoinReq),
    /// Resolve task
    Resolve(ResolveReq),
    /// Finalize task
    Finalize(FinalizeReq),
    /// Stark 2 Snark task
    Snark(SnarkReq),
    /// Keccak coproc callback req
    Keccak(KeccakReq),
}

impl TaskType {
    /// Converts a task type to its string representation
    #[must_use]
    pub fn to_job_type_str(&self) -> String {
        match &self {
            Self::Executor(_) => "executor".into(),
            Self::Prove(_) => "prove-lift".into(),
            Self::Join(_) => "join".into(),
            Self::Resolve(_) => "resolve".into(),
            Self::Finalize(_) => "finalize".into(),
            Self::Snark(_) => "snark".into(),
            Self::Keccak(_) => "keccak".into(),
        }
    }
}
