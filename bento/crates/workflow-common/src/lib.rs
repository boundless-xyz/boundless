// Copyright 2026 Boundless Foundation, Inc.
//
// Use of this source code is governed by the Business Source License
// as found in the LICENSE-BSL file.

use risc0_zkvm::sha::Digest;
use serde::{Deserialize, Serialize};

pub mod metrics;
pub mod s3;

/// Aux worker stream identifier
pub const AUX_WORK_TYPE: &str = "aux";

/// Executor worker stream identifier
pub const EXEC_WORK_TYPE: &str = "exec";

/// prove+lift worker stream identifier
pub const PROVE_WORK_TYPE: &str = "prove";

/// keccak/coproc worker stream identifier
pub const COPROC_WORK_TYPE: &str = "coproc";

/// Keccak receipts directory for job dir
pub const KECCAK_RECEIPT_PATH: &str = "keccak_receipts";

/// join worker stream identifier
pub const JOIN_WORK_TYPE: &str = "join";

/// snark worker stream identifier
pub const SNARK_WORK_TYPE: &str = "snark";

pub const SNARK_RETRIES_DEFAULT: i32 = 3;
pub const SNARK_TIMEOUT_DEFAULT: i32 = 60;

#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq)]
pub enum CompressType {
    None,
    Groth16,
    Blake3Groth16,
}

impl Default for CompressType {
    fn default() -> Self {
        Self::None
    }
}

/// Executor / init request
#[derive(Debug, Deserialize, Serialize)]
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
    /// PoVW log ID if PoVW is enabled, None otherwise
    pub povw_log_id: Option<String>,
    /// PoVW job number if PoVW is enabled, None otherwise
    pub povw_job_number: Option<String>,
}

/// prove + lift task definition
#[derive(Debug, Deserialize, Serialize)]
pub struct ProveReq {
    /// Segment index
    pub index: usize,
}

/// Join Request
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct JoinReq {
    /// Node index
    pub idx: usize,
    /// index of the left branch
    pub left: usize,
    /// index of the right branch
    pub right: usize,
}

/// Union Request
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct UnionReq {
    /// Node index
    pub idx: usize,
    /// index of the left branch
    pub left: usize,
    /// index of the right branch
    pub right: usize,
}

/// Resolve Request
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct ResolveReq {
    /// Index of the final joined receipt
    pub max_idx: usize,
    /// Index of the union task
    pub union_max_idx: Option<usize>,
}

/// Input request
#[derive(Debug, Deserialize, Serialize)]
pub struct FinalizeReq {
    /// Index of the final joined receipt
    pub max_idx: usize,
}

/// Snark task definition
#[derive(Debug, Serialize, Deserialize)]
pub struct SnarkReq {
    /// Stark receipt UUID to pull from minio
    pub receipt: String,
    /// Type of snark compression to run
    pub compress_type: CompressType,
}

/// Snark task response
#[derive(Debug, Serialize, Deserialize)]
pub struct SnarkResp {
    /// Snark UUID in object store snarks/{snark}
    pub snark: String,
}

/// Keccak prove request
#[derive(Debug, Serialize, Deserialize)]
pub struct KeccakReq {
    /// The digest of the claim that this keccak input is expected to produce.
    pub claim_digest: Digest,
    /// The requested size of the keccak proof, in powers of 2.
    pub po2: usize,
    /// The control root which identifies a particular keccak circuit revision.
    pub control_root: Digest,
}

/// High level enum of different sub task types and data
#[derive(Debug, Deserialize, Serialize)]
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
    /// Union task
    Union(UnionReq),
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
            Self::Union(_) => "union".into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_task_type_creation() {
        let resolve_req = ResolveReq { max_idx: 100, union_max_idx: Some(50) };

        // Test that ResolveReq can be created
        assert_eq!(resolve_req.max_idx, 100);
        assert_eq!(resolve_req.union_max_idx, Some(50));
    }

    #[test]
    fn test_join_req_creation() {
        let join_req = JoinReq { idx: 1, left: 2, right: 3 };

        // Test that JoinReq can be created
        assert_eq!(join_req.idx, 1);
        assert_eq!(join_req.left, 2);
        assert_eq!(join_req.right, 3);
    }

    #[test]
    fn test_keccak_request_creation() {
        let keccak_req = KeccakReq {
            claim_digest: Digest::from_bytes([1; 32]),
            po2: 10,
            control_root: Digest::from_bytes([2; 32]),
        };

        // Test that KeccakReq can be created
        assert_eq!(keccak_req.po2, 10);
    }
}
