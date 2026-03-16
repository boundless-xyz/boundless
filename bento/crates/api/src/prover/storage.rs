// Copyright 2026 Boundless Foundation, Inc.
//
// Use of this source code is governed by the Business Source License
// as found in the LICENSE-BSL file.

use crate::prover::types::{CompletedFile, CreateProverReq, JobStatusFile, ProverJobState};
use anyhow::{Context, Result};
use std::{
    fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};
use uuid::Uuid;

const JOBS_DIR: &str = "jobs";
const REQUEST_FILE: &str = "request.json";
const STATUS_FILE: &str = "status.json";
const COMPLETED_FILE: &str = "completed.json";
const RECEIPT_FILE: &str = "receipt.bin";
const ERROR_FILE: &str = "error.txt";

pub fn now_unix_ms() -> u128 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis()
}

pub fn ensure_layout(storage_root: &Path) -> Result<()> {
    fs::create_dir_all(jobs_dir(storage_root)).with_context(|| {
        format!("Failed to create prover jobs dir at {}", jobs_dir(storage_root).display())
    })?;
    Ok(())
}

pub fn jobs_dir(storage_root: &Path) -> PathBuf {
    storage_root.join(JOBS_DIR)
}

pub fn job_dir(storage_root: &Path, job_id: Uuid) -> PathBuf {
    jobs_dir(storage_root).join(job_id.to_string())
}

pub fn status_path(storage_root: &Path, job_id: Uuid) -> PathBuf {
    job_dir(storage_root, job_id).join(STATUS_FILE)
}

pub fn completed_path(storage_root: &Path, job_id: Uuid) -> PathBuf {
    job_dir(storage_root, job_id).join(COMPLETED_FILE)
}

pub fn receipt_path(storage_root: &Path, job_id: Uuid) -> PathBuf {
    job_dir(storage_root, job_id).join(RECEIPT_FILE)
}

pub fn error_path(storage_root: &Path, job_id: Uuid) -> PathBuf {
    job_dir(storage_root, job_id).join(ERROR_FILE)
}

pub fn init_job(
    storage_root: &Path,
    user_id: &str,
    job_id: Uuid,
    request: &CreateProverReq,
) -> Result<JobStatusFile> {
    let job_dir = job_dir(storage_root, job_id);
    fs::create_dir_all(&job_dir)
        .with_context(|| format!("Failed to create job dir {}", job_dir.display()))?;

    write_json_atomic(&job_dir.join(REQUEST_FILE), request)?;

    let now = now_unix_ms();
    let status = JobStatusFile {
        job_id,
        user_id: user_id.to_string(),
        backend: request.backend.clone(),
        state: ProverJobState::Running,
        program_path: request.program_path.clone(),
        input_path: request.input_path.clone(),
        receipt_path: None,
        error_msg: None,
        verify: request.verify,
        created_at_unix_ms: now,
        updated_at_unix_ms: now,
        completed: false,
    };

    write_status(storage_root, &status)?;
    Ok(status)
}

pub fn write_status(storage_root: &Path, status: &JobStatusFile) -> Result<()> {
    write_json_atomic(&status_path(storage_root, status.job_id), status)
}

pub fn read_status(storage_root: &Path, job_id: Uuid) -> Result<JobStatusFile> {
    read_json(&status_path(storage_root, job_id))
}

pub fn write_receipt(storage_root: &Path, job_id: Uuid, receipt_bytes: &[u8]) -> Result<PathBuf> {
    let path = receipt_path(storage_root, job_id);
    write_bytes_atomic(&path, receipt_bytes)?;
    Ok(path)
}

pub fn write_error(storage_root: &Path, job_id: Uuid, error: &str) -> Result<()> {
    write_bytes_atomic(&error_path(storage_root, job_id), error.as_bytes())
}

pub fn mark_completed(
    storage_root: &Path,
    job_id: Uuid,
    backend: crate::prover::types::ProverBackend,
    success: bool,
) -> Result<()> {
    let completed = CompletedFile { job_id, backend, completed_at_unix_ms: now_unix_ms(), success };
    write_json_atomic(&completed_path(storage_root, job_id), &completed)
}

pub fn list_job_ids(storage_root: &Path) -> Result<Vec<Uuid>> {
    let mut out = Vec::new();
    let jobs_dir = jobs_dir(storage_root);
    if !jobs_dir.exists() {
        return Ok(out);
    }

    for entry in fs::read_dir(&jobs_dir)
        .with_context(|| format!("Failed to read jobs dir {}", jobs_dir.display()))?
    {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }

        let Ok(uuid) = Uuid::parse_str(&entry.file_name().to_string_lossy()) else {
            continue;
        };
        out.push(uuid);
    }

    Ok(out)
}

fn write_json_atomic<T: serde::Serialize>(path: &Path, value: &T) -> Result<()> {
    let data = serde_json::to_vec_pretty(value).context("Failed to serialize json")?;
    write_bytes_atomic(path, &data)
}

fn write_bytes_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path
        .parent()
        .with_context(|| format!("Invalid path without parent: {}", path.display()))?;
    fs::create_dir_all(parent)
        .with_context(|| format!("Failed to create parent dir {}", parent.display()))?;

    let tmp_path = path.with_extension("tmp");
    fs::write(&tmp_path, bytes)
        .with_context(|| format!("Failed to write tmp file {}", tmp_path.display()))?;
    fs::rename(&tmp_path, path).with_context(|| {
        format!("Failed to atomically rename {} to {}", tmp_path.display(), path.display())
    })?;
    Ok(())
}

fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T> {
    let data = fs::read(path).with_context(|| format!("Failed to read {}", path.display()))?;
    serde_json::from_slice(&data)
        .with_context(|| format!("Failed to parse json from {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prover::types::ProverBackend;

    struct TempStorageRoot {
        path: PathBuf,
    }

    impl TempStorageRoot {
        fn new() -> Self {
            let path =
                std::env::temp_dir().join(format!("bento_api_prover_storage_{}", Uuid::new_v4()));
            fs::create_dir_all(&path).expect("failed to create temp storage root");
            Self { path }
        }
    }

    impl Drop for TempStorageRoot {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    fn example_request() -> CreateProverReq {
        CreateProverReq {
            backend: ProverBackend::Zisk,
            program_path: "/tmp/program.bin".to_string(),
            input_path: "/tmp/input.bin".to_string(),
            verify: false,
        }
    }

    #[test]
    fn init_job_persists_status_and_request() {
        let tmp = TempStorageRoot::new();
        ensure_layout(&tmp.path).expect("failed to initialize layout");
        let job_id = Uuid::new_v4();
        let req = example_request();

        let status = init_job(&tmp.path, "user_1", job_id, &req).expect("failed to init job");
        assert_eq!(status.job_id, job_id);
        assert!(matches!(status.state, ProverJobState::Running));
        assert!(!status.completed);
        assert!(status_path(&tmp.path, job_id).exists());
        assert!(job_dir(&tmp.path, job_id).join(REQUEST_FILE).exists());

        let reloaded = read_status(&tmp.path, job_id).expect("failed to read status");
        assert_eq!(reloaded.user_id, "user_1");
        assert_eq!(reloaded.program_path, req.program_path);
        assert_eq!(reloaded.input_path, req.input_path);
        assert!(matches!(reloaded.backend, ProverBackend::Zisk));

        let jobs = list_job_ids(&tmp.path).expect("failed to list jobs");
        assert!(jobs.contains(&job_id));
    }

    #[test]
    fn mark_completed_and_artifacts_are_written() {
        let tmp = TempStorageRoot::new();
        ensure_layout(&tmp.path).expect("failed to initialize layout");
        let job_id = Uuid::new_v4();
        init_job(&tmp.path, "user_2", job_id, &example_request()).expect("failed to init job");

        let receipt_path =
            write_receipt(&tmp.path, job_id, b"receipt-bytes").expect("failed to write receipt");
        assert_eq!(receipt_path, super::receipt_path(&tmp.path, job_id));
        assert_eq!(
            fs::read(super::receipt_path(&tmp.path, job_id)).expect("failed to read receipt"),
            b"receipt-bytes"
        );

        write_error(&tmp.path, job_id, "some error").expect("failed to write error");
        assert_eq!(
            fs::read_to_string(error_path(&tmp.path, job_id)).expect("failed to read error"),
            "some error"
        );

        mark_completed(&tmp.path, job_id, ProverBackend::Zisk, true)
            .expect("failed to write completed marker");
        let completed: CompletedFile =
            read_json(&completed_path(&tmp.path, job_id)).expect("failed to read completed marker");
        assert_eq!(completed.job_id, job_id);
        assert!(matches!(completed.backend, ProverBackend::Zisk));
        assert!(completed.success);
    }
}
