// Copyright 2026 Boundless Foundation, Inc.
//
// Use of this source code is governed by the Business Source License
// as found in the LICENSE-BSL file.

use crate::prover::{
    backend::{ExecutionJob, run_default_prover},
    storage,
    types::{JobStatusFile, ProverJobState},
};
use anyhow::{Context, Result};
use std::path::PathBuf;
use uuid::Uuid;

pub async fn run_job(storage_root: PathBuf, job_id: Uuid) -> Result<()> {
    let mut status = storage::read_status(&storage_root, job_id)
        .with_context(|| format!("Failed to load job status for {job_id}"))?;

    let execution_job = ExecutionJob {
        backend: status.backend.clone(),
        program_path: PathBuf::from(&status.program_path),
        input_path: PathBuf::from(&status.input_path),
        verify: status.verify,
    };

    let prove_result = tokio::task::spawn_blocking(move || run_default_prover(&execution_job))
        .await
        .context("Failed to join prover execution thread")?;

    match prove_result {
        Ok(receipt_bytes) => {
            storage::write_receipt(&storage_root, job_id, &receipt_bytes)
                .context("Failed to write local receipt file")?;
            status.state = ProverJobState::Done;
            status.receipt_path =
                Some(storage::receipt_path(&storage_root, job_id).display().to_string());
            status.error_msg = None;
            status.updated_at_unix_ms = storage::now_unix_ms();
            status.completed = true;
            storage::write_status(&storage_root, &status)
                .context("Failed to write done status file")?;
            storage::mark_completed(&storage_root, job_id, status.backend.clone(), true)
                .context("Failed to write completed marker")?;
            Ok(())
        }
        Err(err) => {
            mark_failed(&storage_root, status, &format!("{err:#}"))?;
            Err(err)
        }
    }
}

pub fn recover_running_jobs(storage_root: &std::path::Path) -> Result<()> {
    for job_id in storage::list_job_ids(storage_root)? {
        let status_path = storage::status_path(storage_root, job_id);
        if !status_path.exists() {
            continue;
        }

        let status: JobStatusFile = storage::read_status(storage_root, job_id)?;
        if matches!(status.state, ProverJobState::Running) {
            mark_failed(
                storage_root,
                status,
                "Job was interrupted by API restart before completion",
            )?;
        }
    }

    Ok(())
}

fn mark_failed(storage_root: &std::path::Path, mut status: JobStatusFile, msg: &str) -> Result<()> {
    let mut error_msg = msg.to_string();
    error_msg.truncate(4096);

    storage::write_error(storage_root, status.job_id, &error_msg)
        .context("Failed to write local error file")?;

    status.state = ProverJobState::Failed;
    status.error_msg = Some(error_msg);
    status.receipt_path = None;
    status.updated_at_unix_ms = storage::now_unix_ms();
    status.completed = true;
    storage::write_status(storage_root, &status).context("Failed to write failed status file")?;
    storage::mark_completed(storage_root, status.job_id, status.backend, false)
        .context("Failed to write completed marker")?;
    Ok(())
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::prover::{
        backend::test_env_lock,
        types::{CompletedFile, CreateProverReq, ProverBackend},
    };
    use std::{fs, os::unix::fs::PermissionsExt, path::PathBuf};

    struct TempDir {
        path: PathBuf,
    }

    impl TempDir {
        fn new(prefix: &str) -> Self {
            let path = std::env::temp_dir().join(format!("{prefix}_{}", Uuid::new_v4()));
            fs::create_dir_all(&path).expect("failed to create temp dir");
            Self { path }
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    fn write_executable(path: &std::path::Path, contents: &str) {
        fs::write(path, contents).expect("failed to write script");
        let mut perms = fs::metadata(path).expect("failed to read script metadata").permissions();
        perms.set_mode(0o755);
        fs::set_permissions(path, perms).expect("failed to set script permissions");
    }

    fn create_job_request(program: &str, input: &str) -> CreateProverReq {
        CreateProverReq {
            backend: ProverBackend::Sp1,
            program_path: program.to_string(),
            input_path: input.to_string(),
            verify: false,
        }
    }

    fn read_completed(path: &std::path::Path) -> CompletedFile {
        let data = fs::read(path).expect("failed to read completed marker");
        serde_json::from_slice(&data).expect("failed to parse completed marker")
    }

    fn set_sp1_prover_bin(path: &std::path::Path) {
        // SAFETY: tests serialize environment mutation with `test_env_lock`.
        unsafe { std::env::set_var("SP1_PROVER_BIN", path) };
    }

    fn clear_sp1_prover_bin() {
        // SAFETY: tests serialize environment mutation with `test_env_lock`.
        unsafe { std::env::remove_var("SP1_PROVER_BIN") };
    }

    #[tokio::test]
    async fn run_job_marks_done_and_writes_receipt() {
        let _env_guard = test_env_lock().lock().expect("env mutex poisoned");
        let tmp = TempDir::new("bento_api_prover_runner_ok");
        storage::ensure_layout(&tmp.path).expect("failed to initialize layout");

        let program = tmp.path.join("program.bin");
        let input = tmp.path.join("input.bin");
        fs::write(&program, b"program").expect("failed to write program");
        fs::write(&input, b"input").expect("failed to write input");

        let prover_bin = tmp.path.join("fake-sp1-prover");
        write_executable(
            &prover_bin,
            r#"#!/usr/bin/env sh
set -eu
out=""
while [ "$#" -gt 0 ]; do
  if [ "$1" = "--output" ]; then
    shift
    out="$1"
  fi
  shift
done
printf 'runner-success' > "$out"
"#,
        );
        set_sp1_prover_bin(&prover_bin);

        let job_id = Uuid::new_v4();
        storage::init_job(
            &tmp.path,
            "test-user",
            job_id,
            &create_job_request(&program.display().to_string(), &input.display().to_string()),
        )
        .expect("failed to init job");

        run_job(tmp.path.clone(), job_id).await.expect("run_job should succeed");
        clear_sp1_prover_bin();

        let status = storage::read_status(&tmp.path, job_id).expect("failed to read status");
        assert!(matches!(status.state, ProverJobState::Done));
        assert!(status.completed);
        assert!(status.error_msg.is_none());
        assert!(status.receipt_path.is_some());
        assert_eq!(
            fs::read(storage::receipt_path(&tmp.path, job_id)).expect("failed to read receipt"),
            b"runner-success"
        );

        let completed = read_completed(&storage::completed_path(&tmp.path, job_id));
        assert!(completed.success);
    }

    #[tokio::test]
    async fn run_job_marks_failed_when_prover_fails() {
        let _env_guard = test_env_lock().lock().expect("env mutex poisoned");
        let tmp = TempDir::new("bento_api_prover_runner_fail");
        storage::ensure_layout(&tmp.path).expect("failed to initialize layout");

        let program = tmp.path.join("program.bin");
        let input = tmp.path.join("input.bin");
        fs::write(&program, b"program").expect("failed to write program");
        fs::write(&input, b"input").expect("failed to write input");

        let prover_bin = tmp.path.join("fake-sp1-prover");
        write_executable(
            &prover_bin,
            r#"#!/usr/bin/env sh
set -eu
echo "intentional failure" >&2
exit 42
"#,
        );
        set_sp1_prover_bin(&prover_bin);

        let job_id = Uuid::new_v4();
        storage::init_job(
            &tmp.path,
            "test-user",
            job_id,
            &create_job_request(&program.display().to_string(), &input.display().to_string()),
        )
        .expect("failed to init job");

        let err =
            run_job(tmp.path.clone(), job_id).await.expect_err("run_job should report failure");
        clear_sp1_prover_bin();

        let err_text = format!("{err:#}");
        assert!(err_text.contains("intentional failure"));

        let status = storage::read_status(&tmp.path, job_id).expect("failed to read status");
        assert!(matches!(status.state, ProverJobState::Failed));
        assert!(status.completed);
        assert!(status.error_msg.is_some());
        assert!(status.receipt_path.is_none());

        let error_file = fs::read_to_string(storage::error_path(&tmp.path, job_id))
            .expect("failed to read error file");
        assert!(error_file.contains("intentional failure"));
        assert!(!storage::receipt_path(&tmp.path, job_id).exists());

        let completed = read_completed(&storage::completed_path(&tmp.path, job_id));
        assert!(!completed.success);
    }

    #[test]
    fn recover_running_jobs_marks_interrupted_jobs_failed() {
        let tmp = TempDir::new("bento_api_prover_runner_recover");
        storage::ensure_layout(&tmp.path).expect("failed to initialize layout");

        let job_id = Uuid::new_v4();
        storage::init_job(
            &tmp.path,
            "test-user",
            job_id,
            &create_job_request("/tmp/program.bin", "/tmp/input.bin"),
        )
        .expect("failed to init job");

        recover_running_jobs(&tmp.path).expect("recover_running_jobs failed");

        let status = storage::read_status(&tmp.path, job_id).expect("failed to read status");
        assert!(matches!(status.state, ProverJobState::Failed));
        assert!(status.completed);
        assert!(
            status
                .error_msg
                .unwrap_or_default()
                .contains("interrupted by API restart before completion")
        );

        let completed = read_completed(&storage::completed_path(&tmp.path, job_id));
        assert!(!completed.success);
    }
}
