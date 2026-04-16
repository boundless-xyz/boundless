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

//! Bonsai-compatible HTTP API.
//!
//! Mirrors the REST surface exposed by <https://api.bonsai.xyz> so that the
//! stock `bonsai-sdk` crate can drive this executor without modification.
//! Only the execute-only flow is supported — proving requests are rejected.
//!
//! A non-standard extension: [`ProofReq`] accepts an optional `zkvm` field.
//! When absent it defaults to `"risc0"` (for compatibility with the upstream
//! SDK). The [`Executor`](crate::backend::Executor) / [`Registry`](crate::backend::Registry)
//! plumbing lets additional backends be registered without touching this
//! layer; clients can then pick one by name via the `zkvm` field.

use std::sync::Arc;

use axum::{
    body::Bytes,
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{delete, get, post, put},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    backend::{ExecutorError, Registry, ZkvmKind},
    session::{spawn_session, SessionPhase, SessionRecord},
    storage::Storage,
};

/// Application state shared across all handlers.
#[derive(Clone)]
pub struct AppState {
    pub registry: Arc<Registry>,
    pub storage: Storage,
    /// Base URL we advertise in presigned-style upload responses. The
    /// `bonsai-sdk` client treats this as an opaque URL and issues `PUT`
    /// requests against it, so we route it right back at ourselves.
    pub public_url: String,
}

pub fn router(state: AppState) -> Router {
    Router::new()
        // Meta
        .route("/health", get(health))
        .route("/version", get(version))
        .route("/user/quotas", get(quotas))
        // Images
        .route("/images/upload/{image_id}", get(get_image_upload_url).put(put_image))
        .route("/images/{image_id}", delete(delete_image))
        // Inputs
        .route("/inputs/upload", get(get_input_upload_url))
        .route("/inputs/upload/{input_id}", put(put_input))
        .route("/inputs/{input_id}", delete(delete_input))
        // Receipts (assumptions)
        .route("/receipts/upload", get(get_receipt_upload_url))
        .route("/receipts/upload/{receipt_id}", put(put_receipt))
        .route("/receipts/{session_id}", get(get_receipt_download_url))
        // Sessions
        .route("/sessions/create", post(create_session))
        .route("/sessions/status/{session_id}", get(session_status))
        .route("/sessions/logs/{session_id}", get(session_logs))
        .route("/sessions/stop/{session_id}", get(session_stop))
        .route("/sessions/exec_only_journal/{session_id}", get(session_exec_only_journal))
        // Snark (reject — we only do execute-only)
        .route("/snark/create", post(reject_snark))
        .route("/snark/status/{_uuid}", get(reject_snark))
        .with_state(state)
}

// ---------------------------------------------------------------------------
// Meta
// ---------------------------------------------------------------------------

async fn health() -> &'static str {
    "ok"
}

#[derive(Serialize)]
struct VersionInfo {
    /// Supported versions of the `risc0-zkvm` crate. We deliberately list a
    /// permissive set so clients built against recent risc0 releases pass the
    /// SDK version check.
    risc0_zkvm: Vec<String>,
}

async fn version() -> Json<VersionInfo> {
    Json(VersionInfo {
        risc0_zkvm: vec![
            "1.2".into(),
            "1.3".into(),
            "1.4".into(),
            "2.0".into(),
            "2.1".into(),
            "2.2".into(),
            "3.0".into(),
        ],
    })
}

#[derive(Serialize)]
struct Quotas {
    exec_cycle_limit: i64,
    concurrent_proofs: i64,
    cycle_budget: i64,
    cycle_usage: i64,
    dedicated_executor: i32,
    dedicated_gpu: i32,
}

async fn quotas() -> Json<Quotas> {
    // Static "unlimited-ish" values — this service is intended for local /
    // trusted use and does not meter cycles.
    Json(Quotas {
        exec_cycle_limit: i64::MAX / 2,
        concurrent_proofs: 1024,
        cycle_budget: i64::MAX / 2,
        cycle_usage: 0,
        dedicated_executor: 0,
        dedicated_gpu: 0,
    })
}

// ---------------------------------------------------------------------------
// Images
// ---------------------------------------------------------------------------

#[derive(Serialize)]
struct ImgUploadRes {
    url: String,
}

/// `GET /images/upload/{image_id}`
///
/// Returns a URL the client should PUT the ELF to, or 204 if we already have
/// this image. Matches the behaviour the bonsai-sdk expects in `upload_img`.
async fn get_image_upload_url(
    State(state): State<AppState>,
    Path(image_id): Path<String>,
) -> Response {
    if state.storage.image_exists(&image_id) {
        return StatusCode::NO_CONTENT.into_response();
    }
    let url = format!("{}/images/upload/{}", state.public_url, image_id);
    (StatusCode::OK, Json(ImgUploadRes { url })).into_response()
}

/// `PUT /images/upload/{image_id}` — body is the raw ELF/program bytes.
async fn put_image(
    State(state): State<AppState>,
    Path(image_id): Path<String>,
    body: Bytes,
) -> StatusCode {
    state.storage.put_image(image_id, body);
    StatusCode::OK
}

async fn delete_image(State(state): State<AppState>, Path(image_id): Path<String>) -> StatusCode {
    if state.storage.delete_image(&image_id) {
        StatusCode::OK
    } else {
        StatusCode::NOT_FOUND
    }
}

// ---------------------------------------------------------------------------
// Inputs
// ---------------------------------------------------------------------------

#[derive(Serialize)]
struct UploadRes {
    url: String,
    uuid: String,
}

async fn get_input_upload_url(State(state): State<AppState>) -> Json<UploadRes> {
    let uuid = Uuid::new_v4().to_string();
    let url = format!("{}/inputs/upload/{}", state.public_url, uuid);
    Json(UploadRes { url, uuid })
}

async fn put_input(
    State(state): State<AppState>,
    Path(input_id): Path<String>,
    body: Bytes,
) -> StatusCode {
    state.storage.put_input(input_id, body);
    StatusCode::OK
}

async fn delete_input(State(state): State<AppState>, Path(input_id): Path<String>) -> StatusCode {
    if state.storage.delete_input(&input_id) {
        StatusCode::OK
    } else {
        StatusCode::NOT_FOUND
    }
}

// ---------------------------------------------------------------------------
// Receipts (assumptions)
// ---------------------------------------------------------------------------

async fn get_receipt_upload_url(State(state): State<AppState>) -> Json<UploadRes> {
    let uuid = Uuid::new_v4().to_string();
    let url = format!("{}/receipts/upload/{}", state.public_url, uuid);
    Json(UploadRes { url, uuid })
}

async fn put_receipt(
    State(state): State<AppState>,
    Path(receipt_id): Path<String>,
    body: Bytes,
) -> StatusCode {
    state.storage.put_receipt(receipt_id, body);
    StatusCode::OK
}

/// `GET /receipts/{session_id}`
///
/// In real Bonsai this returns a presigned URL pointing at a serialized
/// `risc0::Receipt`. We don't produce receipts (execute-only), so we 404.
/// Clients that only care about the journal should hit
/// `/sessions/exec_only_journal/{session_id}` instead.
async fn get_receipt_download_url(
    State(_state): State<AppState>,
    Path(_session_id): Path<String>,
) -> Response {
    (StatusCode::NOT_FOUND, Json(serde_json::json!({
        "error": "receipts are not produced by this server (execute-only). use /sessions/exec_only_journal/{id}"
    }))).into_response()
}

// ---------------------------------------------------------------------------
// Sessions
// ---------------------------------------------------------------------------

/// Request body for `POST /sessions/create`. Mirrors `bonsai_sdk::responses::ProofReq`
/// with one non-standard extension: the optional `zkvm` selector.
#[derive(Deserialize)]
struct ProofReq {
    img: String,
    input: String,
    #[serde(default)]
    assumptions: Vec<String>,
    execute_only: bool,
    #[serde(default)]
    exec_cycle_limit: Option<u64>,
    /// Non-standard: pick a backend. Defaults to `risc0` for bonsai-sdk
    /// compatibility.
    #[serde(default = "default_zkvm")]
    zkvm: ZkvmKind,
}

fn default_zkvm() -> ZkvmKind {
    ZkvmKind::Risc0
}

#[derive(Serialize)]
struct CreateSessRes {
    uuid: String,
}

async fn create_session(
    State(state): State<AppState>,
    Json(req): Json<ProofReq>,
) -> Result<Json<CreateSessRes>, ApiError> {
    if !req.execute_only {
        return Err(ApiError::bad_request(
            "this server only supports execute_only=true (no prover installed)",
        ));
    }

    if !state.storage.image_exists(&req.img) {
        return Err(ApiError::bad_request(format!("unknown image id: {}", req.img)));
    }
    if state.storage.get_input(&req.input).is_none() {
        return Err(ApiError::bad_request(format!("unknown input id: {}", req.input)));
    }
    if state.registry.get(req.zkvm).is_none() {
        return Err(ApiError(ExecutorError::BackendDisabled(req.zkvm)));
    }

    let _ = req.assumptions; // accepted but unused (execute-only)
    let _ = req.exec_cycle_limit; // informational

    let session = SessionRecord::new(req.img, req.input, req.zkvm, req.execute_only);
    let uuid = session.uuid.clone();
    state.storage.put_session(session.clone());
    spawn_session(session, state.storage.clone(), state.registry.clone());

    Ok(Json(CreateSessRes { uuid }))
}

#[derive(Serialize)]
struct SessionStats {
    segments: usize,
    total_cycles: u64,
    cycles: u64,
}

#[derive(Serialize)]
struct SessionStatusRes {
    status: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    receipt_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error_msg: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    state: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    elapsed_time: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stats: Option<SessionStats>,
}

async fn session_status(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
) -> Result<Json<SessionStatusRes>, ApiError> {
    let session = lookup_session(&state, &session_id)?;
    let phase = session.phase().await;
    let elapsed = session.elapsed().await.as_secs_f64();

    let res = match phase {
        SessionPhase::Running { stage } => SessionStatusRes {
            status: "RUNNING",
            receipt_url: None,
            error_msg: None,
            state: Some(stage.to_string()),
            elapsed_time: Some(elapsed),
            stats: None,
        },
        SessionPhase::Succeeded { stats } => SessionStatusRes {
            status: "SUCCEEDED",
            // Point at the journal download. Real Bonsai points at a Receipt
            // blob; here it's the raw journal. Clients that check only for
            // presence still work.
            receipt_url: Some(format!(
                "{}/sessions/exec_only_journal/{}",
                state.public_url, session.uuid
            )),
            error_msg: None,
            state: None,
            elapsed_time: Some(elapsed),
            stats: Some(SessionStats {
                // We don't surface segment count from the Executor trait,
                // report 0 until we extend ExecutionStats.
                segments: 0,
                total_cycles: stats.total_cycles,
                cycles: stats.user_cycles,
            }),
        },
        SessionPhase::Failed { error } => SessionStatusRes {
            status: "FAILED",
            receipt_url: None,
            error_msg: Some(error),
            state: None,
            elapsed_time: Some(elapsed),
            stats: None,
        },
        SessionPhase::Aborted => SessionStatusRes {
            status: "ABORTED",
            receipt_url: None,
            error_msg: None,
            state: None,
            elapsed_time: Some(elapsed),
            stats: None,
        },
    };
    Ok(Json(res))
}

async fn session_logs(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
) -> Result<String, ApiError> {
    let session = lookup_session(&state, &session_id)?;
    Ok(session.logs().await)
}

async fn session_stop(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
) -> Result<StatusCode, ApiError> {
    let session = lookup_session(&state, &session_id)?;
    session.mark_aborted().await;
    Ok(StatusCode::OK)
}

async fn session_exec_only_journal(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
) -> Result<Vec<u8>, ApiError> {
    let session = lookup_session(&state, &session_id)?;
    match session.phase().await {
        SessionPhase::Succeeded { stats } => {
            let journal = stats
                .journal_hex
                .as_deref()
                .map(|h| hex::decode(h).unwrap_or_default())
                .unwrap_or_default();
            Ok(journal)
        }
        SessionPhase::Running { .. } => Err(ApiError::bad_request("session still running")),
        SessionPhase::Failed { error } => Err(ApiError::bad_request(error)),
        SessionPhase::Aborted => Err(ApiError::bad_request("session aborted")),
    }
}

// ---------------------------------------------------------------------------
// Snark (unsupported)
// ---------------------------------------------------------------------------

async fn reject_snark() -> ApiError {
    ApiError::bad_request("snark proving is not supported by this server")
}

// ---------------------------------------------------------------------------
// Helpers / errors
// ---------------------------------------------------------------------------

fn lookup_session(state: &AppState, id: &str) -> Result<Arc<SessionRecord>, ApiError> {
    state
        .storage
        .get_session(id)
        .ok_or_else(|| ApiError::not_found(format!("unknown session id: {id}")))
}

#[derive(Debug)]
pub struct ApiError(pub ExecutorError);

impl ApiError {
    fn bad_request(msg: impl Into<String>) -> Self {
        Self(ExecutorError::InvalidInput(msg.into()))
    }

    fn not_found(msg: impl Into<String>) -> Self {
        Self(ExecutorError::Execution(anyhow::Error::msg(format!("not found: {}", msg.into()))))
    }
}

impl From<ExecutorError> for ApiError {
    fn from(e: ExecutorError) -> Self {
        Self(e)
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let status = match &self.0 {
            ExecutorError::BackendDisabled(_) => StatusCode::BAD_REQUEST,
            ExecutorError::InvalidElf(_) | ExecutorError::InvalidInput(_) => {
                StatusCode::BAD_REQUEST
            }
            ExecutorError::Execution(e) if e.to_string().starts_with("not found:") => {
                StatusCode::NOT_FOUND
            }
            ExecutorError::Execution(_) => StatusCode::INTERNAL_SERVER_ERROR,
        };
        let body = Json(serde_json::json!({ "error": self.0.to_string() }));
        (status, body).into_response()
    }
}
