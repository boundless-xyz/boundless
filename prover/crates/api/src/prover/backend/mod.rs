// Copyright 2026 Boundless Foundation, Inc.
//
// Use of this source code is governed by the Business Source License
// as found in the LICENSE-BSL file.

mod openvm;
mod risc0;
mod zisk;

use crate::prover::types::ProverBackend;
use anyhow::{Context, Result, anyhow, bail};
use std::{path::PathBuf, process::Command};
use uuid::Uuid;

#[derive(Clone)]
pub struct ExecutionJob {
    pub backend: ProverBackend,
    pub program_path: PathBuf,
    pub input_path: PathBuf,
    pub verify: bool,
}

pub fn run_default_prover(job: &ExecutionJob) -> Result<Vec<u8>> {
    match job.backend {
        ProverBackend::Risc0 => risc0::prove(job),
        ProverBackend::Zisk => zisk::prove(job),
        ProverBackend::Openvm => openvm::prove(job),
    }
}

fn run_cli_backend(job: &ExecutionJob, binary_env: &str, binary_fallback: &str) -> Result<Vec<u8>> {
    let binary = std::env::var(binary_env).unwrap_or_else(|_| binary_fallback.to_string());
    let output_path =
        std::env::temp_dir().join(format!("{}_{}.bin", job.backend.as_str(), Uuid::new_v4()));

    let mut command = Command::new(&binary);
    command
        .arg("prove")
        .arg("--program")
        .arg(&job.program_path)
        .arg("--input")
        .arg(&job.input_path)
        .arg("--output")
        .arg(&output_path);
    if job.verify {
        command.arg("--verify");
    }

    let output =
        command.output().with_context(|| format!("Failed to execute backend binary `{binary}`"))?;
    if !output.status.success() {
        bail!(
            "{} prover command failed with status {:?}. stderr: {}",
            job.backend.as_str(),
            output.status.code(),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    if output_path.exists() {
        let receipt_bytes = std::fs::read(&output_path).with_context(|| {
            format!(
                "Failed to read {} prover output file {}",
                job.backend.as_str(),
                output_path.display()
            )
        })?;
        let _ = std::fs::remove_file(&output_path);
        return Ok(receipt_bytes);
    }

    if output.stdout.is_empty() {
        return Err(anyhow!(
            "{} prover produced no output file and no stdout bytes",
            job.backend.as_str()
        ));
    }

    Ok(output.stdout)
}

#[cfg(test)]
pub(crate) fn test_env_lock() -> &'static std::sync::Mutex<()> {
    static LOCK: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
    LOCK.get_or_init(|| std::sync::Mutex::new(()))
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::{
        fs,
        os::unix::fs::PermissionsExt,
        path::{Path, PathBuf},
    };

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

    fn write_executable(path: &Path, contents: &str) {
        fs::write(path, contents).expect("failed to write script");
        let mut perms = fs::metadata(path).expect("failed to read script metadata").permissions();
        perms.set_mode(0o755);
        fs::set_permissions(path, perms).expect("failed to set script permissions");
    }

    fn zisk_job() -> ExecutionJob {
        ExecutionJob {
            backend: ProverBackend::Zisk,
            program_path: PathBuf::from("/tmp/program.bin"),
            input_path: PathBuf::from("/tmp/input.bin"),
            verify: false,
        }
    }

    fn set_zisk_prover_bin(path: &Path) {
        // SAFETY: tests serialize environment mutation with `test_env_lock`.
        unsafe { std::env::set_var("ZISK_PROVER_BIN", path) };
    }

    fn clear_zisk_prover_bin() {
        // SAFETY: tests serialize environment mutation with `test_env_lock`.
        unsafe { std::env::remove_var("ZISK_PROVER_BIN") };
    }

    #[test]
    fn cli_backend_reads_output_file() {
        let _env_guard = test_env_lock().lock().expect("env mutex poisoned");
        let tmp = TempDir::new("bento_api_prover_backend_ok");
        let bin_path = tmp.path.join("fake-zisk-prover");
        write_executable(
            &bin_path,
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
printf 'file-receipt' > "$out"
"#,
        );

        set_zisk_prover_bin(&bin_path);
        let receipt = run_default_prover(&zisk_job()).expect("expected successful prove");
        clear_zisk_prover_bin();

        assert_eq!(receipt, b"file-receipt");
    }

    #[test]
    fn cli_backend_returns_stdout_when_no_output_file_exists() {
        let _env_guard = test_env_lock().lock().expect("env mutex poisoned");
        let tmp = TempDir::new("bento_api_prover_backend_stdout");
        let bin_path = tmp.path.join("fake-zisk-prover");
        write_executable(
            &bin_path,
            r#"#!/usr/bin/env sh
set -eu
printf 'stdout-receipt'
"#,
        );

        set_zisk_prover_bin(&bin_path);
        let receipt = run_default_prover(&zisk_job()).expect("expected successful prove");
        clear_zisk_prover_bin();

        assert_eq!(receipt, b"stdout-receipt");
    }

    #[test]
    fn cli_backend_surfaces_command_failure() {
        let _env_guard = test_env_lock().lock().expect("env mutex poisoned");
        let tmp = TempDir::new("bento_api_prover_backend_fail");
        let bin_path = tmp.path.join("fake-zisk-prover");
        write_executable(
            &bin_path,
            r#"#!/usr/bin/env sh
set -eu
echo "boom from prover" >&2
exit 9
"#,
        );

        set_zisk_prover_bin(&bin_path);
        let err = run_default_prover(&zisk_job()).expect_err("expected prove failure");
        clear_zisk_prover_bin();

        let err_text = format!("{err:#}");
        assert!(err_text.contains("boom from prover"));
        assert!(err_text.contains("zisk prover command failed"));
    }
}
