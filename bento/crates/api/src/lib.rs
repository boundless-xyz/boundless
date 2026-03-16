// Copyright 2026 Boundless Foundation, Inc.
//
// Use of this source code is governed by the Business Source License
// as found in the LICENSE-BSL file.

use anyhow::{Context, Error as AnyhowErr, Result};
use axum::{
    Json, Router, async_trait,
    body::{Body, to_bytes},
    extract::{FromRequestParts, Host, Path, Query, State},
    http::{StatusCode, header::CONTENT_TYPE, request::Parts},
    response::{IntoResponse, Response},
    routing::{delete, get, post, put},
};
use bonsai_sdk::responses::{
    CreateSessRes, ImgUploadRes, ReceiptDownload, SessionStats, SessionStatusRes, SnarkStatusRes,
    UploadRes,
};
use clap::Parser;
use deadpool_redis::{Config as RedisConfig, Pool as RedisPool, Runtime, redis::AsyncCommands};
use risc0_zkvm::compute_image_id;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use taskdb::{JobState, Priority, ReadyTask, TaskDb, TaskDbErr};
use thiserror::Error;
use uuid::Uuid;
use workflow_common::{
    COPROC_WORK_TYPE, CompressType, ExecutorReq, JOIN_WORK_TYPE, PROVE_WORK_TYPE,
    SNARK_RETRIES_DEFAULT, SNARK_TIMEOUT_DEFAULT, SNARK_WORK_TYPE, SnarkReq as WorkflowSnarkReq,
    TaskType,
    s3::{
        BLAKE3_GROTH16_BUCKET_DIR, ELF_BUCKET_DIR, GROTH16_BUCKET_DIR, INPUT_BUCKET_DIR,
        PREFLIGHT_JOURNALS_BUCKET_DIR, RECEIPT_BUCKET_DIR, S3Client, STARK_BUCKET_DIR,
        WORK_RECEIPTS_BUCKET_DIR,
    },
};

mod helpers;

#[derive(Debug, Deserialize, Serialize)]
pub struct ErrMsg {
    pub r#type: String,
    pub msg: String,
}
impl ErrMsg {
    pub fn new(r#type: &str, msg: &str) -> Self {
        Self { r#type: r#type.into(), msg: msg.into() }
    }
}
impl std::fmt::Display for ErrMsg {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "error_type: {} msg: {}", self.r#type, self.msg)
    }
}

#[derive(Debug, Deserialize, Serialize)]
pub struct WorkReceiptInfo {
    pub key: String,
    /// PoVW log ID if PoVW is enabled, None otherwise
    pub povw_log_id: Option<String>,
    /// PoVW job number if PoVW is enabled, None otherwise
    pub povw_job_number: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct WorkReceiptList {
    pub receipts: Vec<WorkReceiptInfo>,
}

#[derive(Debug, Deserialize, Serialize)]
struct WorkerTask {
    job_id: String,
    task_id: String,
    task_def: serde_json::Value,
    prereqs: serde_json::Value,
    max_retries: i32,
}

#[derive(Debug, Deserialize, Serialize)]
struct TaskUpdateRes {
    updated: bool,
}

#[derive(Debug, Deserialize, Serialize)]
struct TaskDoneReq {
    output: serde_json::Value,
}

#[derive(Debug, Deserialize, Serialize)]
struct TaskFailedReq {
    error: String,
}

#[derive(Debug, Deserialize)]
struct WorkerClaimQuery {
    wait_timeout_secs: Option<u64>,
}

#[derive(Debug, Deserialize, Serialize)]
struct TaskRetriesRunningRes {
    retries: Option<i32>,
}

#[derive(Debug, Deserialize)]
struct HotBlobPutQuery {
    ttl_secs: Option<u64>,
}

#[derive(Debug, Deserialize, Serialize)]
struct StarkApiReq {
    img: String,
    input: String,
    #[serde(default)]
    assumptions: Vec<String>,
    #[serde(default)]
    execute_only: bool,
    exec_cycle_limit: Option<u64>,
    #[serde(default)]
    priority: Priority,
}

#[derive(Debug, Deserialize, Serialize)]
struct SnarkApiReq {
    session_id: String,
    #[serde(default)]
    priority: Priority,
}

fn is_gpu_worker_stream(task_stream: &str) -> bool {
    matches!(
        task_stream,
        PROVE_WORK_TYPE | JOIN_WORK_TYPE | COPROC_WORK_TYPE | SNARK_WORK_TYPE
    )
}

impl From<ReadyTask> for WorkerTask {
    fn from(task: ReadyTask) -> Self {
        Self {
            job_id: task.job_id.to_string(),
            task_id: task.task_id,
            task_def: task.task_def,
            prereqs: task.prereqs,
            max_retries: task.max_retries,
        }
    }
}

pub struct ExtractApiKey(pub String);

#[async_trait]
impl<S> FromRequestParts<S> for ExtractApiKey
where
    S: Send + Sync,
{
    type Rejection = (StatusCode, &'static str);

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        let Some(api_key_header) = parts.headers.get("x-api-key") else {
            return Ok(ExtractApiKey(USER_ID.to_string()));
            // return Err((StatusCode::UNAUTHORIZED, "Missing x-api-key header"));
        };

        let api_key_str = match api_key_header.to_str() {
            Ok(res) => res,
            Err(err) => {
                tracing::error!("Failed to deserialize x-api-key header: {err}");
                return Err((StatusCode::INTERNAL_SERVER_ERROR, ""));
            }
        };

        return Ok(ExtractApiKey(api_key_str.to_string()));
    }
}

#[derive(Error, Debug)]
pub enum AppError {
    #[error("the image name is invalid: {0}")]
    ImageInvalid(String),

    #[error("The provided imageid already exists: {0}")]
    ImgAlreadyExists(String),

    #[error("The image id does not match the computed id, req: {0} comp: {1}")]
    ImageIdMismatch(String, String),

    #[error("The provided inputid already exists: {0}")]
    InputAlreadyExists(String),

    #[error("The provided receiptid already exists: {0}")]
    ReceiptAlreadyExists(String),

    #[error("receipt does not exist: {0}")]
    ReceiptMissing(String),

    #[error("preflight journal does not exist: {0}")]
    JournalMissing(String),

    #[error("asset does not exist: {0}")]
    AssetMissing(String),

    #[error("hot data does not exist: {0}")]
    HotDataMissing(String),

    #[error("invalid gpu worker stream: {0}")]
    InvalidGpuWorkerStream(String),

    #[error("Database error")]
    DbError(#[from] TaskDbErr),

    #[error("internal error")]
    InternalErr(AnyhowErr),
}

impl AppError {
    fn type_str(&self) -> String {
        match self {
            Self::ImageInvalid(_) => "ImageInvalid",
            Self::ImgAlreadyExists(_) => "ImgAlreadyExists",
            Self::ImageIdMismatch(_, _) => "ImageIdMismatch",
            Self::InputAlreadyExists(_) => "InputAlreadyExists",
            Self::ReceiptAlreadyExists(_) => "ReceiptAlreadyExists",
            Self::ReceiptMissing(_) => "ReceiptMissing",
            Self::JournalMissing(_) => "JournalMissing",
            Self::AssetMissing(_) => "AssetMissing",
            Self::HotDataMissing(_) => "HotDataMissing",
            Self::InvalidGpuWorkerStream(_) => "InvalidGpuWorkerStream",
            Self::DbError(_) => "DbError",
            Self::InternalErr(_) => "InternalErr",
        }
        .into()
    }
}

impl From<AnyhowErr> for AppError {
    fn from(err: AnyhowErr) -> Self {
        Self::InternalErr(err)
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let code = match self {
            Self::ImageInvalid(_) | Self::ImageIdMismatch(_, _) => StatusCode::BAD_REQUEST,
            Self::ImgAlreadyExists(_)
            | Self::InputAlreadyExists(_)
            | Self::ReceiptAlreadyExists(_) => StatusCode::NO_CONTENT,
            Self::InvalidGpuWorkerStream(_) => StatusCode::BAD_REQUEST,
            Self::ReceiptMissing(_)
            | Self::JournalMissing(_)
            | Self::AssetMissing(_)
            | Self::HotDataMissing(_) => StatusCode::NOT_FOUND,
            Self::InternalErr(_) | Self::DbError(_) => StatusCode::INTERNAL_SERVER_ERROR,
        };

        match self {
            Self::ImgAlreadyExists(_) => tracing::debug!("Image exists: {code}, {self:?}"),
            // Missing objects can happen during normal polling / race windows.
            Self::ReceiptMissing(_)
            | Self::JournalMissing(_)
            | Self::AssetMissing(_)
            | Self::HotDataMissing(_) => tracing::warn!("api miss, code {code}: {self:?}"),
            _ => tracing::error!("api error, code {code}: {self:?}"),
        }

        (code, Json(ErrMsg { r#type: self.type_str(), msg: self.to_string() })).into_response()
    }
}

#[derive(Parser, Debug)]
#[clap(author, version, about, long_about = None)]
pub struct Args {
    /// Bind address for REST api
    #[clap(long, default_value = "0.0.0.0:8081")]
    bind_addr: String,

    /// taskdb redis URL (defaults to `redis_url`)
    #[clap(env, long)]
    taskdb_redis_url: Option<String>,

    /// redis taskdb namespace prefix
    #[clap(env, long, default_value = "taskdb")]
    taskdb_redis_namespace: String,

    /// Local disk path used as object storage root.
    #[clap(env, long, default_value = "./data/object_store")]
    storage_dir: String,

    /// redis connection URL used for hot worker data
    #[clap(env, long)]
    redis_url: String,

    /// Executor timeout in seconds
    #[clap(long, default_value_t = 4 * 60 * 60)]
    exec_timeout: i32,

    /// Executor retries
    #[clap(long, default_value_t = 0)]
    exec_retries: i32,

    /// Snark timeout in seconds
    #[clap(long, default_value_t = SNARK_TIMEOUT_DEFAULT)]
    snark_timeout: i32,

    /// Snark retries
    #[clap(long, default_value_t = SNARK_RETRIES_DEFAULT)]
    snark_retries: i32,

}

pub struct AppState {
    task_db: TaskDb,
    redis_pool: RedisPool,
    s3_client: S3Client,
    exec_timeout: i32,
    exec_retries: i32,
    snark_timeout: i32,
    snark_retries: i32,
}

impl AppState {
    pub async fn new(args: &Args) -> Result<Arc<Self>> {
        let taskdb_redis_url = args.taskdb_redis_url.as_deref().unwrap_or(&args.redis_url);
        let task_db = TaskDb::connect_redis(taskdb_redis_url, &args.taskdb_redis_namespace)
            .context("Failed to initialize redis taskdb backend")?;

        let s3_client = S3Client::from_local_dir(&args.storage_dir)
            .context("Failed to initialize local object storage")?;
        let redis_pool = RedisConfig::from_url(&args.redis_url)
            .create_pool(Some(Runtime::Tokio1))
            .context("Failed to initialize redis pool")?;

        Ok(Arc::new(Self {
            task_db,
            redis_pool,
            s3_client,
            exec_timeout: args.exec_timeout,
            exec_retries: args.exec_retries,
            snark_timeout: args.snark_timeout,
            snark_retries: args.snark_retries,
        }))
    }
}

// TODO: Add authn/z to get a userID
const USER_ID: &str = "default_user";
// No limit on upload size given API is trusted.
const MAX_UPLOAD_SIZE: usize = usize::MAX;

const IMAGE_UPLOAD_PATH: &str = "/images/upload/:image_id";
async fn image_upload(
    State(state): State<Arc<AppState>>,
    Path(image_id): Path<String>,
    Host(hostname): Host,
) -> Result<Json<ImgUploadRes>, AppError> {
    let new_img_key = format!("{ELF_BUCKET_DIR}/{image_id}");
    if state
        .s3_client
        .object_exists(&new_img_key)
        .await
        .context("Failed to check if object exists")?
    {
        return Err(AppError::ImgAlreadyExists(image_id));
    }

    Ok(Json(ImgUploadRes { url: format!("http://{hostname}/images/upload/{image_id}") }))
}

async fn image_upload_put(
    State(state): State<Arc<AppState>>,
    Path(image_id): Path<String>,
    body: Body,
) -> Result<(), AppError> {
    let new_img_key = format!("{ELF_BUCKET_DIR}/{image_id}");
    if state
        .s3_client
        .object_exists(&new_img_key)
        .await
        .context("Failed to check if object exists")?
    {
        return Err(AppError::ImgAlreadyExists(image_id));
    }

    let body_bytes =
        to_bytes(body, MAX_UPLOAD_SIZE).await.context("Failed to convert body to bytes")?;

    let computed = compute_image_id(&body_bytes)
        .ok()
        .map(|id| id.to_string())
        .unwrap_or_else(|| "<invalid-risc0-image>".to_string());
    if computed != image_id {
        return Err(AppError::ImageIdMismatch(image_id, computed));
    }

    state
        .s3_client
        .write_buf_to_s3(&new_img_key, body_bytes.to_vec())
        .await
        .context("Failed to upload image to object store")?;

    Ok(())
}

const INPUT_UPLOAD_PATH: &str = "/inputs/upload";
async fn input_upload(
    State(state): State<Arc<AppState>>,
    Host(hostname): Host,
) -> Result<Json<UploadRes>, AppError> {
    let input_id = Uuid::new_v4();

    let new_img_key = format!("{INPUT_BUCKET_DIR}/{input_id}");
    if state
        .s3_client
        .object_exists(&new_img_key)
        .await
        .context("Failed to check if object exists")?
    {
        return Err(AppError::InputAlreadyExists(input_id.to_string()));
    }

    Ok(Json(UploadRes {
        url: format!("http://{hostname}/inputs/upload/{input_id}"),
        uuid: input_id.to_string(),
    }))
}

const INPUT_UPLOAD_PUT_PATH: &str = "/inputs/upload/:input_id";
async fn input_upload_put(
    State(state): State<Arc<AppState>>,
    Path(input_id): Path<String>,
    body: Body,
) -> Result<(), AppError> {
    let new_input_key = format!("{INPUT_BUCKET_DIR}/{input_id}");
    if state
        .s3_client
        .object_exists(&new_input_key)
        .await
        .context("Failed to check if object exists")?
    {
        return Err(AppError::InputAlreadyExists(input_id.to_string()));
    }

    // TODO: Support streaming uploads
    let body_bytes =
        to_bytes(body, MAX_UPLOAD_SIZE).await.context("Failed to convert body to bytes")?;
    state
        .s3_client
        .write_buf_to_s3(&new_input_key, body_bytes.to_vec())
        .await
        .context("Failed to upload input to object store")?;

    Ok(())
}

const RECEIPT_UPLOAD_PATH: &str = "/receipts/upload";
async fn receipt_upload(
    State(state): State<Arc<AppState>>,
    Host(hostname): Host,
) -> Result<Json<UploadRes>, AppError> {
    let receipt_id = Uuid::new_v4();
    let new_receipt_key = format!("{RECEIPT_BUCKET_DIR}/{STARK_BUCKET_DIR}/{receipt_id}.bincode");
    if state
        .s3_client
        .object_exists(&new_receipt_key)
        .await
        .context("Failed to check if object exists")?
    {
        return Err(AppError::InputAlreadyExists(receipt_id.to_string()));
    }

    Ok(Json(UploadRes {
        url: format!("http://{hostname}/receipts/upload/{receipt_id}"),
        uuid: receipt_id.to_string(),
    }))
}

const RECEIPT_UPLOAD_PUT_PATH: &str = "/receipts/upload/:receipt_id";
async fn receipt_upload_put(
    State(state): State<Arc<AppState>>,
    Path(receipt_id): Path<String>,
    body: Body,
) -> Result<(), AppError> {
    let new_receipt_key = format!("{RECEIPT_BUCKET_DIR}/{STARK_BUCKET_DIR}/{receipt_id}.bincode");
    if state
        .s3_client
        .object_exists(&new_receipt_key)
        .await
        .context("Failed to check if object exists")?
    {
        return Err(AppError::InputAlreadyExists(receipt_id.to_string()));
    }

    // TODO: Support streaming uploads
    let body_bytes =
        to_bytes(body, MAX_UPLOAD_SIZE).await.context("Failed to convert body to bytes")?;
    state
        .s3_client
        .write_buf_to_s3(&new_receipt_key, body_bytes.to_vec())
        .await
        .context("Failed to upload receipt to object store")?;

    Ok(())
}

// Stark routes

const STARK_PROVING_START_PATH: &str = "/sessions/create";
async fn prove_stark(
    State(state): State<Arc<AppState>>,
    ExtractApiKey(api_key): ExtractApiKey,
    Json(start_req): Json<StarkApiReq>,
) -> Result<Json<CreateSessRes>, AppError> {
    let (_aux_stream, exec_stream, _gpu_prove_stream, _gpu_coproc_stream, _gpu_join_stream) =
        helpers::get_or_create_streams(&state.task_db, &api_key)
            .await
            .context("Failed to get / create steams")?;

    let task_def = serde_json::to_value(TaskType::Executor(ExecutorReq {
        image: start_req.img,
        input: start_req.input,
        user_id: api_key.clone(),
        assumptions: start_req.assumptions,
        execute_only: start_req.execute_only,
        compress: workflow_common::CompressType::None,
        exec_limit: start_req.exec_cycle_limit,
    }))
    .context("Failed to serialize ExecutorReq")?;

    let job_id = state
        .task_db
        .create_job_with_priority(
            &exec_stream,
            &task_def,
            state.exec_retries,
            state.exec_timeout,
            &api_key,
            start_req.priority,
        )
        .await
        .context("Failed to create exec / init task")?;

    Ok(Json(CreateSessRes { uuid: job_id.to_string() }))
}

const STARK_STATUS_PATH: &str = "/sessions/status/:job_id";
async fn stark_status(
    State(state): State<Arc<AppState>>,
    Host(hostname): Host,
    Path(job_id): Path<Uuid>,
    ExtractApiKey(api_key): ExtractApiKey,
) -> Result<Json<SessionStatusRes>, AppError> {
    let job_state_result = state.task_db.get_job_state(&job_id, &api_key).await;

    // If job not found in taskdb, check object storage for completed receipt.
    if let Err(err) = &job_state_result
        && err.is_not_found()
    {
        let receipt_key = format!("{RECEIPT_BUCKET_DIR}/{STARK_BUCKET_DIR}/{job_id}.bincode");
        if state
            .s3_client
            .object_exists(&receipt_key)
            .await
            .context("Failed to check if receipt exists")?
        {
            // Receipt exists - job was completed and cleaned up from DB
            return Ok(Json(SessionStatusRes {
                state: Some("".into()),
                receipt_url: Some(format!("http://{hostname}/receipts/stark/receipt/{job_id}")),
                error_msg: None,
                status: JobState::Done.to_string(),
                elapsed_time: None,
                stats: None,
            }));
        }
    }

    // Job exists in DB or doesn't exist anywhere - return DB result
    let job_state = job_state_result.context("Failed to get job state")?;

    let (exec_stats, receipt_url) = if job_state == JobState::Done {
        let exec_stats = helpers::get_exec_stats(&state.task_db, &job_id)
            .await
            .context("Failed to get exec stats")?;
        (
            Some(SessionStats {
                cycles: exec_stats.user_cycles,
                segments: exec_stats.segments as usize,
                total_cycles: exec_stats.total_cycles,
            }),
            Some(format!("http://{hostname}/receipts/stark/receipt/{job_id}")),
        )
    } else {
        (None, None)
    };

    let error_msg = if job_state == JobState::Failed {
        Some(
            state
                .task_db
                .get_job_failure(&job_id)
                .await
                .context("Failed to get job error message")?,
        )
    } else {
        None
    };

    Ok(Json(SessionStatusRes {
        state: Some("".into()), // TODO
        receipt_url,
        error_msg,
        status: job_state.to_string(),
        elapsed_time: None,
        stats: exec_stats,
    }))
}

const GET_STARK_PATH: &str = "/receipts/stark/receipt/:job_id";

async fn stark_download(
    State(state): State<Arc<AppState>>,
    Path(job_id): Path<Uuid>,
) -> Result<Vec<u8>, AppError> {
    let receipt_key = format!("{RECEIPT_BUCKET_DIR}/{STARK_BUCKET_DIR}/{job_id}.bincode");
    if !state
        .s3_client
        .object_exists(&receipt_key)
        .await
        .context("Failed to check if object exists")?
    {
        return Err(AppError::ReceiptMissing(job_id.to_string()));
    }

    let receipt = state
        .s3_client
        .read_buf_from_s3(&receipt_key)
        .await
        .context("Failed to read from object store")?;

    Ok(receipt)
}

const RECEIPT_DOWNLOAD_PATH: &str = "/receipts/:job_id";
async fn receipt_download(
    State(state): State<Arc<AppState>>,
    Path(job_id): Path<Uuid>,
    Host(hostname): Host,
) -> Result<Json<ReceiptDownload>, AppError> {
    let receipt_key = format!("{RECEIPT_BUCKET_DIR}/{STARK_BUCKET_DIR}/{job_id}.bincode");
    if !state
        .s3_client
        .object_exists(&receipt_key)
        .await
        .context("Failed to check if object exists")?
    {
        return Err(AppError::ReceiptMissing(job_id.to_string()));
    }

    Ok(Json(ReceiptDownload { url: format!("http://{hostname}/receipts/stark/receipt/{job_id}") }))
}

const GET_JOURNAL_PATH: &str = "/sessions/exec_only_journal/:job_id";
async fn preflight_journal(
    State(state): State<Arc<AppState>>,
    Path(job_id): Path<Uuid>,
) -> Result<Vec<u8>, AppError> {
    let journal_key = format!("{PREFLIGHT_JOURNALS_BUCKET_DIR}/{job_id}.bin");
    if !state
        .s3_client
        .object_exists(&journal_key)
        .await
        .context("Failed to check if object exists")?
    {
        return Err(AppError::JournalMissing(job_id.to_string()));
    }

    let receipt = state
        .s3_client
        .read_buf_from_s3(&journal_key)
        .await
        .context("Failed to read from object store")?;

    Ok(receipt)
}

// Snark routes

const SNARK_START_PATH: &str = "/snark/create";
async fn prove_groth16(
    State(state): State<Arc<AppState>>,
    ExtractApiKey(api_key): ExtractApiKey,
    Json(start_req): Json<SnarkApiReq>,
) -> Result<Json<CreateSessRes>, AppError> {
    let (_aux_stream, _exec_stream, gpu_prove_stream, _gpu_coproc_stream, _gpu_join_stream) =
        helpers::get_or_create_streams(&state.task_db, &api_key)
            .await
            .context("Failed to get / create steams")?;

    let task_def = serde_json::to_value(TaskType::Snark(WorkflowSnarkReq {
        receipt: start_req.session_id,
        compress_type: CompressType::Groth16,
    }))
    .context("Failed to serialize ExecutorReq")?;

    let job_id = state
        .task_db
        .create_job_with_priority(
            &gpu_prove_stream,
            &task_def,
            state.snark_retries,
            state.snark_timeout,
            &api_key,
            start_req.priority,
        )
        .await
        .context("Failed to create exec / init task")?;

    Ok(Json(CreateSessRes { uuid: job_id.to_string() }))
}

const BLAKE3_GROTH16_START_PATH: &str = "/shrink_bitvm2/create";
async fn prove_blake3_groth16(
    State(state): State<Arc<AppState>>,
    ExtractApiKey(api_key): ExtractApiKey,
    Json(start_req): Json<SnarkApiReq>,
) -> Result<Json<CreateSessRes>, AppError> {
    let (_aux_stream, _exec_stream, gpu_prove_stream, _gpu_coproc_stream, _gpu_join_stream) =
        helpers::get_or_create_streams(&state.task_db, &api_key)
            .await
            .context("Failed to get / create steams")?;

    let task_def = serde_json::to_value(TaskType::Snark(WorkflowSnarkReq {
        receipt: start_req.session_id,
        compress_type: CompressType::Blake3Groth16,
    }))
    .context("Failed to serialize ExecutorReq")?;

    let job_id = state
        .task_db
        .create_job_with_priority(
            &gpu_prove_stream,
            &task_def,
            state.snark_retries,
            state.snark_timeout,
            &api_key,
            start_req.priority,
        )
        .await
        .context("Failed to create exec / init task")?;
    Ok(Json(CreateSessRes { uuid: job_id.to_string() }))
}

const BLAKE3_GROTH16_STATUS_PATH: &str = "/shrink_bitvm2/status/:job_id";
async fn blake3_groth16_status(
    State(state): State<Arc<AppState>>,
    ExtractApiKey(api_key): ExtractApiKey,
    Path(job_id): Path<Uuid>,
    Host(hostname): Host,
) -> Result<Json<SnarkStatusRes>, AppError> {
    let job_state =
        state.task_db.get_job_state(&job_id, &api_key).await.context("Failed to get job state")?;
    let (error_msg, output) = match job_state {
        JobState::Running => (None, None),
        JobState::Done => {
            (None, Some(format!("http://{hostname}/receipts/shrink_bitvm2/receipt/{job_id}")))
        }
        JobState::Failed => (
            Some(
                state
                    .task_db
                    .get_job_failure(&job_id)
                    .await
                    .context("Failed to get job error message")?,
            ),
            None,
        ),
    };
    Ok(Json(SnarkStatusRes { status: job_state.to_string(), error_msg, output }))
}

const SNARK_STATUS_PATH: &str = "/snark/status/:job_id";
async fn groth16_status(
    State(state): State<Arc<AppState>>,
    ExtractApiKey(api_key): ExtractApiKey,
    Path(job_id): Path<Uuid>,
    Host(hostname): Host,
) -> Result<Json<SnarkStatusRes>, AppError> {
    let job_state_result = state.task_db.get_job_state(&job_id, &api_key).await;

    // If job not found in taskdb, check object storage for completed receipt.
    if let Err(err) = &job_state_result
        && err.is_not_found()
    {
        let receipt_key = format!("{RECEIPT_BUCKET_DIR}/{GROTH16_BUCKET_DIR}/{job_id}.bincode");
        if state
            .s3_client
            .object_exists(&receipt_key)
            .await
            .context("Failed to check if receipt exists")?
        {
            // Receipt exists - job was completed and cleaned up from taskdb
            return Ok(Json(SnarkStatusRes {
                status: JobState::Done.to_string(),
                error_msg: None,
                output: Some(format!("http://{hostname}/receipts/groth16/receipt/{job_id}")),
            }));
        }
    }

    let job_state = job_state_result.context("Failed to get job state")?;

    let (error_msg, output) = match job_state {
        JobState::Running => (None, None),
        JobState::Done => {
            (None, Some(format!("http://{hostname}/receipts/groth16/receipt/{job_id}")))
        }
        JobState::Failed => (
            Some(
                state
                    .task_db
                    .get_job_failure(&job_id)
                    .await
                    .context("Failed to get job error message")?,
            ),
            None,
        ),
    };
    Ok(Json(SnarkStatusRes { status: job_state.to_string(), error_msg, output }))
}


const GET_GROTH16_PATH: &str = "/receipts/groth16/receipt/:job_id";
async fn groth16_download(
    State(state): State<Arc<AppState>>,
    Path(job_id): Path<Uuid>,
) -> Result<Vec<u8>, AppError> {
    let receipt_key = format!("{RECEIPT_BUCKET_DIR}/{GROTH16_BUCKET_DIR}/{job_id}.bincode");
    if !state
        .s3_client
        .object_exists(&receipt_key)
        .await
        .context("Failed to check if object exists")?
    {
        return Err(AppError::ReceiptMissing(job_id.to_string()));
    }

    let receipt = state
        .s3_client
        .read_buf_from_s3(&receipt_key)
        .await
        .context("Failed to read from object store")?;

    Ok(receipt)
}

const GET_BLAKE3_GROTH16_PATH: &str = "/receipts/shrink_bitvm2/receipt/:job_id";
async fn blake3_groth16_download(
    State(state): State<Arc<AppState>>,
    Path(job_id): Path<Uuid>,
) -> Result<Vec<u8>, AppError> {
    let receipt_key = format!("{RECEIPT_BUCKET_DIR}/{BLAKE3_GROTH16_BUCKET_DIR}/{job_id}.bincode");
    if !state
        .s3_client
        .object_exists(&receipt_key)
        .await
        .context("Failed to check if object exists")?
    {
        return Err(AppError::ReceiptMissing(job_id.to_string()));
    }

    let receipt = state
        .s3_client
        .read_buf_from_s3(&receipt_key)
        .await
        .context("Failed to read from object store")?;

    Ok(receipt)
}

const GET_WORK_RECEIPT_PATH: &str = "/work-receipts/:receipt_id";
async fn get_work_receipt(
    State(state): State<Arc<AppState>>,
    Path(receipt_id): Path<String>,
) -> Result<Vec<u8>, AppError> {
    let receipt_key = format!("{WORK_RECEIPTS_BUCKET_DIR}/{receipt_id}.bincode");
    if !state
        .s3_client
        .object_exists(&receipt_key)
        .await
        .context("Failed to check if object exists")?
    {
        return Err(AppError::ReceiptMissing(receipt_id));
    }

    let receipt = state
        .s3_client
        .read_buf_from_s3(&receipt_key)
        .await
        .context("Failed to read from object store")?;

    Ok(receipt)
}

const GET_ASSET_PATH: &str = "/assets/*object_key";
async fn asset_download(
    State(state): State<Arc<AppState>>,
    Path(object_key): Path<String>,
) -> Result<Response, AppError> {
    if !state
        .s3_client
        .object_exists(&object_key)
        .await
        .context("Failed to check if asset exists")?
    {
        return Err(AppError::AssetMissing(object_key));
    }

    let asset = state
        .s3_client
        .read_buf_from_s3(&object_key)
        .await
        .context("Failed to read asset from object store")?;

    Ok(([(CONTENT_TYPE, "application/octet-stream")], asset).into_response())
}

const GPU_WORK_CLAIM_PATH: &str = "/worker/gpu/tasks/claim/:task_stream";
async fn claim_gpu_work(
    State(state): State<Arc<AppState>>,
    Path(task_stream): Path<String>,
    Query(query): Query<WorkerClaimQuery>,
) -> Result<Json<Option<WorkerTask>>, AppError> {
    if !is_gpu_worker_stream(&task_stream) {
        return Err(AppError::InvalidGpuWorkerStream(task_stream));
    }

    let wait_timeout_secs = query.wait_timeout_secs.unwrap_or_default();
    let task = state
        .task_db
        .request_work_wait(&task_stream, wait_timeout_secs)
        .await
        .context("Failed to claim GPU work")?
        .map(WorkerTask::from);

    Ok(Json(task))
}

const GPU_TASK_DONE_PATH: &str = "/worker/gpu/tasks/:job_id/:task_id/done";
async fn gpu_task_done(
    State(state): State<Arc<AppState>>,
    Path((job_id, task_id)): Path<(Uuid, String)>,
    Json(req): Json<TaskDoneReq>,
) -> Result<Json<TaskUpdateRes>, AppError> {
    Ok(Json(TaskUpdateRes {
        updated: state
            .task_db
            .update_task_done(&job_id, &task_id, req.output)
            .await
            .context("Failed to update GPU task as done")?,
    }))
}

const GPU_TASK_FAILED_PATH: &str = "/worker/gpu/tasks/:job_id/:task_id/failed";
async fn gpu_task_failed(
    State(state): State<Arc<AppState>>,
    Path((job_id, task_id)): Path<(Uuid, String)>,
    Json(req): Json<TaskFailedReq>,
) -> Result<Json<TaskUpdateRes>, AppError> {
    Ok(Json(TaskUpdateRes {
        updated: state
            .task_db
            .update_task_failed(&job_id, &task_id, &req.error)
            .await
            .context("Failed to update GPU task as failed")?,
    }))
}

const GPU_TASK_RETRY_PATH: &str = "/worker/gpu/tasks/:job_id/:task_id/retry";
async fn gpu_task_retry(
    State(state): State<Arc<AppState>>,
    Path((job_id, task_id)): Path<(Uuid, String)>,
) -> Result<Json<TaskUpdateRes>, AppError> {
    Ok(Json(TaskUpdateRes {
        updated: state
            .task_db
            .update_task_retry(&job_id, &task_id)
            .await
            .context("Failed to retry GPU task")?,
    }))
}

const GPU_TASK_RETRIES_RUNNING_PATH: &str = "/worker/gpu/tasks/:job_id/:task_id/retries-running";
async fn gpu_task_retries_running(
    State(state): State<Arc<AppState>>,
    Path((job_id, task_id)): Path<(Uuid, String)>,
) -> Result<Json<TaskRetriesRunningRes>, AppError> {
    Ok(Json(TaskRetriesRunningRes {
        retries: state
            .task_db
            .get_task_retries_running(&job_id, &task_id)
            .await
            .context("Failed to fetch GPU task retries")?,
    }))
}

const WORKER_HOT_PATH: &str = "/worker/hot/*key";
async fn worker_hot_get(
    State(state): State<Arc<AppState>>,
    Path(key): Path<String>,
) -> Result<Response, AppError> {
    let mut conn = state.redis_pool.get().await.context("Failed to acquire redis connection")?;
    let value: Option<Vec<u8>> = conn.get(&key).await.context("Failed to fetch hot data")?;
    let value = value.ok_or_else(|| AppError::HotDataMissing(key))?;
    Ok(([(CONTENT_TYPE, "application/octet-stream")], value).into_response())
}

async fn worker_hot_put(
    State(state): State<Arc<AppState>>,
    Path(key): Path<String>,
    Query(query): Query<HotBlobPutQuery>,
    body: Body,
) -> Result<StatusCode, AppError> {
    let body_bytes =
        to_bytes(body, MAX_UPLOAD_SIZE).await.context("Failed to read hot data request body")?;
    let mut conn = state.redis_pool.get().await.context("Failed to acquire redis connection")?;
    if let Some(ttl_secs) = query.ttl_secs {
        conn.set_ex::<_, _, ()>(&key, body_bytes.to_vec(), ttl_secs)
            .await
            .context("Failed to store hot data with TTL")?;
    } else {
        conn.set::<_, _, ()>(&key, body_bytes.to_vec())
            .await
            .context("Failed to store hot data")?;
    }
    Ok(StatusCode::NO_CONTENT)
}

async fn worker_hot_delete(
    State(state): State<Arc<AppState>>,
    Path(key): Path<String>,
) -> Result<StatusCode, AppError> {
    let mut conn = state.redis_pool.get().await.context("Failed to acquire redis connection")?;
    conn.unlink::<_, ()>(&key).await.context("Failed to delete hot data")?;
    Ok(StatusCode::NO_CONTENT)
}

const LIST_WORK_RECEIPTS_PATH: &str = "/work-receipts";
async fn list_work_receipts(
    State(state): State<Arc<AppState>>,
) -> Result<Json<WorkReceiptList>, AppError> {
    // List all objects in the work receipts bucket
    let objects = state
        .s3_client
        .list_objects(Some(WORK_RECEIPTS_BUCKET_DIR))
        .await
        .context("Failed to list work receipt objects")?;

    tracing::info!("Found {} objects in work receipts bucket", objects.len());
    let mut receipts = Vec::new();

    for object_key in objects {
        tracing::debug!("Processing object {}", object_key);
        // Extract the receipt ID from the object key
        // Object keys are in format: "work_receipts/{receipt_id}.bincode"
        if let Some(receipt_id) = object_key.strip_prefix(&format!("{WORK_RECEIPTS_BUCKET_DIR}/"))
            && receipt_id.ends_with(".bincode")
            && !receipt_id.ends_with("_povw.bincode")
        {
            let receipt_id = receipt_id.trim_end_matches(".bincode");

            // Try to extract POVW information from the stored receipt
            let mut povw_log_id = None;
            let mut povw_job_number = None;

            // First check if there's a metadata file (this should exist for all receipts)
            let metadata_key = format!("{WORK_RECEIPTS_BUCKET_DIR}/{receipt_id}_metadata.json");
            if let Ok(metadata_bytes) = state.s3_client.read_buf_from_s3(&metadata_key).await {
                if let Ok(metadata) = serde_json::from_slice::<serde_json::Value>(&metadata_bytes) {
                    povw_log_id =
                        metadata.get("povw_log_id").and_then(|v| v.as_str()).map(|s| s.to_string());
                    povw_job_number = metadata
                        .get("povw_job_number")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string());

                    tracing::debug!(
                        "Found metadata for {}: log_id={:?}, job_number={:?}",
                        receipt_id,
                        povw_log_id,
                        povw_job_number
                    );
                }
            } else {
                tracing::debug!("No metadata file found for receipt: {}", receipt_id);
            }

            // Check if there's a corresponding POVW receipt
            let povw_key = format!("{WORK_RECEIPTS_BUCKET_DIR}/{receipt_id}_povw.bincode");
            let has_povw_receipt = state.s3_client.object_exists(&povw_key).await.unwrap_or(false);

            if has_povw_receipt {
                tracing::debug!("POVW receipt found for: {}", receipt_id);

                // If we don't have metadata but have a POVW receipt, try to extract from the receipt
                if povw_log_id.is_none() || povw_job_number.is_none() {
                    match state
                            .s3_client
                            .read_from_s3::<risc0_zkvm::GenericReceipt<
                                risc0_zkvm::WorkClaim<risc0_zkvm::ReceiptClaim>,
                            >>(&povw_key)
                            .await
                        {
                            Ok(_receipt) => {
                                tracing::debug!(
                                    "Successfully parsed POVW receipt for: {}",
                                    receipt_id
                                );
                                // For now, use receipt_id as fallback values
                                // TODO: Extract actual POVW metadata from receipt when available
                                if povw_log_id.is_none() {
                                    povw_log_id = Some(receipt_id.to_string());
                                }
                                if povw_job_number.is_none() {
                                    povw_job_number = Some(receipt_id.to_string());
                                }
                            }
                            Err(err) => {
                                tracing::warn!(
                                    "Failed to parse POVW receipt for {}: {}",
                                    receipt_id,
                                    err
                                );
                                // Still mark as having POVW but without metadata
                                if povw_log_id.is_none() {
                                    povw_log_id = Some(receipt_id.to_string());
                                }
                                if povw_job_number.is_none() {
                                    povw_job_number = Some(receipt_id.to_string());
                                }
                            }
                        }
                }
            } else {
                tracing::debug!("No POVW receipt found for: {}", receipt_id);
            }

            receipts.push(WorkReceiptInfo {
                key: receipt_id.to_string(),
                povw_log_id,
                povw_job_number,
            });
        }
    }

    tracing::info!("Listed {} work receipts from bucket", receipts.len());
    Ok(Json(WorkReceiptList { receipts }))
}

// Health check endpoint for Docker Compose
const HEALTH_PATH: &str = "/health";
async fn health_check() -> StatusCode {
    StatusCode::OK
}

pub fn app(state: Arc<AppState>) -> Router {
    Router::new()
        .route(HEALTH_PATH, get(health_check))
        .route(IMAGE_UPLOAD_PATH, get(image_upload))
        .route(IMAGE_UPLOAD_PATH, put(image_upload_put))
        .route(INPUT_UPLOAD_PATH, get(input_upload))
        .route(INPUT_UPLOAD_PUT_PATH, put(input_upload_put))
        .route(RECEIPT_UPLOAD_PATH, get(receipt_upload))
        .route(RECEIPT_UPLOAD_PUT_PATH, put(receipt_upload_put))
        .route(STARK_PROVING_START_PATH, post(prove_stark))
        .route(STARK_STATUS_PATH, get(stark_status))
        .route(GET_STARK_PATH, get(stark_download))
        .route(RECEIPT_DOWNLOAD_PATH, get(receipt_download))
        .route(GET_JOURNAL_PATH, get(preflight_journal))
        .route(SNARK_START_PATH, post(prove_groth16))
        .route(SNARK_STATUS_PATH, get(groth16_status))
        .route(BLAKE3_GROTH16_START_PATH, post(prove_blake3_groth16))
        .route(BLAKE3_GROTH16_STATUS_PATH, get(blake3_groth16_status))
        .route(GET_GROTH16_PATH, get(groth16_download))
        .route(GET_BLAKE3_GROTH16_PATH, get(blake3_groth16_download))
        .route(GET_ASSET_PATH, get(asset_download))
        .route(GPU_WORK_CLAIM_PATH, post(claim_gpu_work))
        .route(GPU_TASK_DONE_PATH, post(gpu_task_done))
        .route(GPU_TASK_FAILED_PATH, post(gpu_task_failed))
        .route(GPU_TASK_RETRY_PATH, post(gpu_task_retry))
        .route(GPU_TASK_RETRIES_RUNNING_PATH, get(gpu_task_retries_running))
        .route(WORKER_HOT_PATH, get(worker_hot_get))
        .route(WORKER_HOT_PATH, put(worker_hot_put))
        .route(WORKER_HOT_PATH, delete(worker_hot_delete))
        .route(GET_WORK_RECEIPT_PATH, get(get_work_receipt))
        .route(LIST_WORK_RECEIPTS_PATH, get(list_work_receipts))
        .with_state(state)
        .layer(tower_http::compression::CompressionLayer::new())
}

pub async fn run(args: &Args) -> Result<()> {
    let app_state = AppState::new(args).await.context("Failed to initialize AppState")?;
    let listener = tokio::net::TcpListener::bind(&args.bind_addr)
        .await
        .context("Failed to bind a TCP listener")?;

    tracing::info!("REST API listening on: {}", args.bind_addr);
    axum::serve(listener, self::app(app_state))
        .with_graceful_shutdown(shutdown_signal())
        .await
        .context("REST API service failed")?;

    Ok(())
}

async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c().await.expect("failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("failed to install signal handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
}
