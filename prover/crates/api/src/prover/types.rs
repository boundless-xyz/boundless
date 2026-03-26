// Copyright 2026 Boundless Foundation, Inc.
//
// Use of this source code is governed by the Business Source License
// as found in the LICENSE-BSL file.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProverBackend {
    Risc0,
    Zisk,
    Openvm,
}

impl ProverBackend {
    #[must_use]
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Risc0 => "risc0",
            Self::Zisk => "zisk",
            Self::Openvm => "openvm",
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct CreateProverReq {
    pub backend: ProverBackend,
    pub program_path: String,
    pub input_path: String,
    #[serde(default)]
    pub verify: bool,
}

#[derive(Debug, Serialize)]
pub struct CreateProverRes {
    pub job_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProverJobState {
    Running,
    Done,
    Failed,
}

impl std::fmt::Display for ProverJobState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Running => write!(f, "RUNNING"),
            Self::Done => write!(f, "SUCCEEDED"),
            Self::Failed => write!(f, "FAILED"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobStatusFile {
    pub job_id: Uuid,
    pub user_id: String,
    pub backend: ProverBackend,
    pub state: ProverJobState,
    pub program_path: String,
    pub input_path: String,
    pub receipt_path: Option<String>,
    pub error_msg: Option<String>,
    pub verify: bool,
    pub created_at_unix_ms: u128,
    pub updated_at_unix_ms: u128,
    pub completed: bool,
}

#[derive(Debug, Serialize)]
pub struct ProverStatusRes {
    pub job_id: String,
    pub backend: ProverBackend,
    pub status: String,
    pub receipt_path: Option<String>,
    pub error_msg: Option<String>,
    pub completed: bool,
}

impl From<JobStatusFile> for ProverStatusRes {
    fn from(value: JobStatusFile) -> Self {
        Self {
            job_id: value.job_id.to_string(),
            backend: value.backend,
            status: value.state.to_string(),
            receipt_path: value.receipt_path,
            error_msg: value.error_msg,
            completed: value.completed,
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct CompletedFile {
    pub job_id: Uuid,
    pub backend: ProverBackend,
    pub completed_at_unix_ms: u128,
    pub success: bool,
}
