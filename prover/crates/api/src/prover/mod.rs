// Copyright 2026 Boundless Foundation, Inc.
//
// Use of this source code is governed by the Business Source License
// as found in the LICENSE-BSL file.

pub mod backend;
pub mod runner;
pub mod storage;
pub mod types;

use crate::{AppError, AppState, ExtractApiKey};
use anyhow::Context;
use axum::{
    Json,
    extract::{Path, State},
};
use std::{path::PathBuf, sync::Arc};
use types::{CreateProverReq, CreateProverRes, ProverStatusRes};
use uuid::Uuid;

pub const PROVER_CREATE_PATH: &str = "/prover/create";
pub const PROVER_STATUS_PATH: &str = "/prover/status/:job_id";
pub const PROVER_RECEIPT_PATH: &str = "/prover/receipt/:job_id";

pub async fn prover_create(
    State(state): State<Arc<AppState>>,
    ExtractApiKey(api_key): ExtractApiKey,
    Json(req): Json<CreateProverReq>,
) -> Result<Json<CreateProverRes>, AppError> {
    let program_path = PathBuf::from(&req.program_path);
    if !program_path.is_file() {
        return Err(AppError::ImageInvalid(format!(
            "program_path does not exist: {}",
            program_path.display()
        )));
    }

    let input_path = PathBuf::from(&req.input_path);
    if !input_path.is_file() {
        return Err(AppError::ImageInvalid(format!(
            "input_path does not exist: {}",
            input_path.display()
        )));
    }

    let job_id = Uuid::new_v4();

    storage::init_job(&state.prover_storage_dir, &api_key, job_id, &req)
        .context("Failed to initialize local prover job")?;

    let storage_root = state.prover_storage_dir.clone();
    tokio::spawn(async move {
        if let Err(err) = runner::run_job(storage_root, job_id).await {
            tracing::error!("[BENTO-PROVER-001] Job {job_id} failed: {err:#}");
        }
    });

    Ok(Json(CreateProverRes { job_id: job_id.to_string() }))
}

pub async fn prover_status(
    State(state): State<Arc<AppState>>,
    Path(job_id): Path<Uuid>,
    _api_key: ExtractApiKey,
) -> Result<Json<ProverStatusRes>, AppError> {
    let status = storage::read_status(&state.prover_storage_dir, job_id)
        .map_err(|_| AppError::JobMissing(job_id.to_string()))?;

    Ok(Json(status.into()))
}

pub async fn prover_receipt(
    State(state): State<Arc<AppState>>,
    Path(job_id): Path<Uuid>,
    _api_key: ExtractApiKey,
) -> Result<Vec<u8>, AppError> {
    let status = storage::read_status(&state.prover_storage_dir, job_id)
        .map_err(|_| AppError::JobMissing(job_id.to_string()))?;

    if !matches!(status.state, types::ProverJobState::Done) {
        return Err(AppError::JobIncomplete(job_id.to_string()));
    }

    std::fs::read(storage::receipt_path(&state.prover_storage_dir, job_id))
        .with_context(|| format!("Failed to read local receipt for job {job_id}"))
        .map_err(AppError::from)
}
