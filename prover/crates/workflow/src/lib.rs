// Copyright 2026 Boundless Foundation, Inc.
//
// Use of this source code is governed by the Business Source License
// as found in the LICENSE-BSL file.

#![deny(missing_docs)]

//! Workflow processing Agent service

use crate::{assets::ApiClient, redis::RedisPool};
use anyhow::{Context, Result};
use clap::Parser;
use risc0_zkvm::{ProverOpts, ProverServer, VerifierContext, get_prover_server};
use std::{
    rc::Rc,
    str::FromStr,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Instant,
};
use taskdb::{ReadyTask, TaskDb, TaskStateCount};
use tokio::time;
use workflow_common::{COPROC_WORK_TYPE, TaskType, metrics::helpers};
mod assets;
mod redis;
mod tasks;

pub use workflow_common::{
    AUX_WORK_TYPE, EXEC_WORK_TYPE, JOIN_WORK_TYPE, PROVE_WORK_TYPE, SNARK_RETRIES_DEFAULT,
    SNARK_TIMEOUT_DEFAULT, storage::SharedFs,
};

const TASK_QUEUE_METRICS_REFRESH_INTERVAL_SECS: u64 = 5;

fn uses_gpu_worker_api(task_stream: &str) -> bool {
    matches!(
        task_stream,
        PROVE_WORK_TYPE | JOIN_WORK_TYPE | COPROC_WORK_TYPE | workflow_common::SNARK_WORK_TYPE
    )
}

fn needs_prover_server(task_stream: &str) -> bool {
    matches!(
        task_stream,
        PROVE_WORK_TYPE | JOIN_WORK_TYPE | COPROC_WORK_TYPE | workflow_common::SNARK_WORK_TYPE
    )
}

/// Workflow agent
///
/// Monitors taskdb for new tasks on the selected stream and processes the work.
/// CPU / aux workers use direct taskdb + redis access, while GPU workers use the API.
#[derive(Parser, Debug, Clone)]
#[command(author, version, about, long_about = None)]
pub struct Args {
    /// agent stream type to monitor for tasks
    ///
    /// ex: `cpu`, `prove`, `join`, `snark`, etc
    #[arg(env, short, long)]
    pub task_stream: String,

    /// Polling internal between tasks
    ///
    /// Time to wait between request_work calls
    #[arg(env, short, long, default_value_t = 1)]
    pub poll_time: u64,

    /// taskdb redis URL (defaults to `redis_url`)
    #[clap(env, long)]
    pub taskdb_redis_url: Option<String>,

    /// redis taskdb namespace prefix
    #[clap(env, long, default_value = "taskdb")]
    pub taskdb_redis_namespace: String,

    /// redis connection URL
    #[clap(env)]
    pub redis_url: Option<String>,

    /// risc0 segment po2 arg
    #[clap(env, short, long, default_value_t = 20)]
    pub segment_po2: u32,

    /// Redis TTL, seconds before objects expire automatically
    ///
    /// Defaults to 8 hours
    #[clap(env,long, default_value_t = 8 * 60 * 60)]
    pub redis_ttl: u64,

    /// Executor limit, in millions of cycles
    #[clap(env, short, long, default_value_t = 100_000)]
    pub exec_cycle_limit: u64,

    /// Local disk path used as object storage root.
    #[clap(env, long, default_value = "./data/object_store")]
    pub storage_dir: String,

    /// Base URL for the Bento API used to fetch input assets.
    #[clap(env = "BENTO_API_URL", long, default_value = "http://localhost:8081")]
    pub api_url: String,

    /// Enables a background thread to monitor for tasks that need to be retried / timed-out
    #[clap(env, long, default_value_t = false)]
    monitor_requeue: bool,

    // Task flags
    /// How many times a prove+lift can fail before hard failure
    #[clap(env, long, default_value_t = 3)]
    prove_retries: i32,

    /// How long can a prove+lift can be running for, before it is marked as timed-out
    #[clap(env, long, default_value_t = 30)]
    prove_timeout: i32,

    /// How many times a join can fail before hard failure
    #[clap(env, long, default_value_t = 3)]
    join_retries: i32,

    /// How long can a join can be running for, before it is marked as timed-out
    #[clap(env, long, default_value_t = 10)]
    join_timeout: i32,

    /// How many times a resolve can fail before hard failure
    #[clap(env, long, default_value_t = 3)]
    resolve_retries: i32,

    /// How long can a resolve can be running for, before it is marked as timed-out
    #[clap(env, long, default_value_t = 10)]
    resolve_timeout: i32,

    /// How many times a finalize can fail before hard failure
    #[clap(env, long, default_value_t = 0)]
    finalize_retries: i32,

    /// How long can a finalize can be running for, before it is marked as timed-out
    ///
    /// NOTE: This value is multiplied by the assumption count
    #[clap(env, long, default_value_t = 10)]
    finalize_timeout: i32,

    /// Snark timeout in seconds
    #[clap(env, long, default_value_t = SNARK_TIMEOUT_DEFAULT)]
    snark_timeout: i32,

    /// Snark retries
    #[clap(env, long, default_value_t = SNARK_RETRIES_DEFAULT)]
    snark_retries: i32,

    /// Requeue poll interval in seconds
    ///
    /// How often to check for tasks that need to be requeued
    #[clap(env, long, default_value_t = 5)]
    requeue_poll_interval: u64,

    /// Stuck tasks poll interval in seconds
    ///
    /// How often to check for stuck pending tasks
    #[clap(env, long, default_value_t = 60)]
    stuck_tasks_poll_interval: u64,

    /// Completed job cleanup poll interval in seconds
    ///
    /// How often to clean up completed jobs
    #[clap(env, long, default_value_t = 5 * 60)]
    cleanup_poll_interval: u64,

    /// Disable cron to clean up completed jobs in taskdb.
    #[clap(long, default_value_t = false, env = "BENTO_DISABLE_COMPLETED_CLEANUP")]
    disable_completed_cleanup: bool,

    /// Disable cron to clean up stuck tasks in taskdb.
    #[clap(long, env = "BENTO_DISABLE_STUCK_TASK_CLEANUP")]
    disable_stuck_task_cleanup: bool,
}

/// Core agent context to hold all optional clients / pools and state
pub struct Agent {
    /// Redis-backed taskdb client.
    task_db: Option<TaskDb>,
    /// segment po2 config
    pub segment_po2: u32,
    /// redis connection pool
    redis_pool: Option<RedisPool>,
    /// Shared filesystem storage client
    pub storage: SharedFs,
    /// API-backed worker client used for assets and GPU task execution
    api_client: ApiClient,
    /// all configuration params:
    args: Args,
    /// risc0 Prover server
    prover: Option<Rc<dyn ProverServer>>,
    /// risc0 verifier context
    verifier_ctx: VerifierContext,
    /// Last time shared task queue metrics were refreshed.
    queue_metrics_last_refresh: Mutex<Option<Instant>>,
}

impl Agent {
    /// Check if POVW is enabled for this agent instance
    pub fn is_povw_enabled(&self) -> bool {
        std::env::var("POVW_LOG_ID").is_ok()
    }

    /// Update database metrics.
    pub fn update_db_pool_metrics(&self) {
        // Taskdb is Redis-only; SQL pool metrics are obsolete.
    }

    /// Returns the direct taskdb handle when this worker is configured to use one.
    pub fn direct_task_db(&self) -> Option<&TaskDb> {
        self.task_db.as_ref()
    }

    pub(crate) fn task_db(&self) -> Result<&TaskDb> {
        self.task_db
            .as_ref()
            .context("[BENTO-WF-200] This worker stream is API-backed and has no direct taskdb")
    }

    pub(crate) fn redis_pool(&self) -> Result<&RedisPool> {
        self.redis_pool
            .as_ref()
            .context("[BENTO-WF-201] This worker stream is API-backed and has no direct redis")
    }

    /// Initialize the [Agent] from the [Args] config params
    ///
    /// Starts any connection pools and establishes the agents configs
    pub async fn new(args: Args) -> Result<Self> {
        // Validate POVW environment variables at startup
        if let Ok(log_id) = std::env::var("POVW_LOG_ID") {
            risc0_binfmt::PovwLogId::from_str(&log_id)
                .map_err(|e| anyhow::anyhow!("Failed to parse POVW_LOG_ID: {}", e))?;
            tracing::info!("POVW enabled with log_id: {}", log_id);
        } else {
            tracing::debug!("POVW disabled");
        }

        let api_backed_gpu_worker = uses_gpu_worker_api(&args.task_stream);
        let task_db = if api_backed_gpu_worker {
            None
        } else {
            let redis_taskdb_url = args
                .taskdb_redis_url
                .clone()
                .or_else(|| args.redis_url.clone())
                .context("[BENTO-WF-100] TASKDB_REDIS_URL or REDIS_URL is required")?;
            let task_db = TaskDb::connect_redis(&redis_taskdb_url, &args.taskdb_redis_namespace)
                .context("[BENTO-WF-100] Failed to initialize redis taskdb backend")?;
            Some(task_db)
        };
        let redis_pool = if api_backed_gpu_worker {
            None
        } else {
            Some(crate::redis::create_pool(
                args.redis_url
                    .as_deref()
                    .context("[BENTO-WF-101] REDIS_URL is required for non-GPU worker streams")?,
            )?)
        };
        let api_client = ApiClient::new(&args.api_url)
            .context("[BENTO-WF-102] Failed to initialize API client")?;
        let storage = SharedFs::new(&args.storage_dir)
            .context("[BENTO-WF-103] Failed to initialize shared storage")?;

        let verifier_ctx = VerifierContext::default();
        let prover = if needs_prover_server(&args.task_stream) {
            let opts = ProverOpts::default();
            let prover = get_prover_server(&opts)
                .context("[BENTO-WF-104] Failed to initialize prover server")?;
            Some(prover)
        } else {
            None
        };

        Ok(Self {
            task_db,
            segment_po2: args.segment_po2,
            redis_pool,
            storage,
            api_client,
            args,
            prover,
            verifier_ctx,
            queue_metrics_last_refresh: Mutex::new(None),
        })
    }

    fn task_type_label(&self, task: &ReadyTask) -> String {
        let task_type: TaskType = match serde_json::from_value(task.task_def.clone()) {
            Ok(task_type) => task_type,
            Err(_) => return "invalid_task".to_string(),
        };

        match task_type {
            TaskType::Join(_) if self.is_povw_enabled() => "join_povw".to_string(),
            TaskType::Resolve(_) if self.is_povw_enabled() => "resolve_povw".to_string(),
            other => other.to_job_type_str(),
        }
    }

    fn should_refresh_queue_metrics(&self) -> bool {
        let mut last_refresh =
            self.queue_metrics_last_refresh.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let now = Instant::now();
        if let Some(last) = *last_refresh
            && now.duration_since(last).as_secs() < TASK_QUEUE_METRICS_REFRESH_INTERVAL_SECS
        {
            return false;
        }
        *last_refresh = Some(now);
        true
    }

    async fn refresh_task_queue_metrics(&self) -> Result<()> {
        let Some(task_db) = self.direct_task_db() else {
            return Ok(());
        };
        if !self.should_refresh_queue_metrics() {
            return Ok(());
        }

        let counts = task_db
            .get_task_state_counts()
            .await
            .context("[BENTO-WF-209] Failed to fetch task state counts for metrics")?;
        Self::publish_task_queue_metrics(&counts);
        Ok(())
    }

    fn publish_task_queue_metrics(counts: &[TaskStateCount]) {
        helpers::reset_task_queue_depth();
        for count in counts {
            helpers::set_task_queue_depth(
                &count.worker_type,
                count.priority.as_metric_label(),
                &count.state,
                count.count as i64,
            );
        }
    }

    async fn request_work_direct(&self) -> Result<Option<ReadyTask>> {
        if uses_gpu_worker_api(&self.args.task_stream) {
            self.api_client
                .claim_gpu_work(&self.args.task_stream, self.args.poll_time)
                .await
                .context("[BENTO-WF-105] Failed to claim GPU work from API")
        } else {
            self.task_db()?
                .request_work_wait(&self.args.task_stream, self.args.poll_time)
                .await
                .context("[BENTO-WF-107] Failed to request_work")
        }
    }

    async fn request_work(&self) -> Result<Option<ReadyTask>> {
        self.request_work_direct().await
    }

    async fn update_task_done(
        &self,
        job_id: &uuid::Uuid,
        task_id: &str,
        output: serde_json::Value,
    ) -> Result<bool> {
        if uses_gpu_worker_api(&self.args.task_stream) {
            self.api_client
                .update_task_done(job_id, task_id, output)
                .await
                .context("[BENTO-WF-133] Failed to report task done through API")
        } else {
            self.task_db()?
                .update_task_done(job_id, task_id, output)
                .await
                .context("[BENTO-WF-133] Failed to report task done")
        }
    }

    async fn update_task_failed(
        &self,
        job_id: &uuid::Uuid,
        task_id: &str,
        error: &str,
    ) -> Result<bool> {
        if uses_gpu_worker_api(&self.args.task_stream) {
            self.api_client
                .update_task_failed(job_id, task_id, error)
                .await
                .context("[BENTO-WF-110] Failed to report task failure through API")
        } else {
            self.task_db()?
                .update_task_failed(job_id, task_id, error)
                .await
                .context("[BENTO-WF-110] Failed to report task failure")
        }
    }

    async fn update_task_retry(&self, job_id: &uuid::Uuid, task_id: &str) -> Result<bool> {
        if uses_gpu_worker_api(&self.args.task_stream) {
            self.api_client
                .update_task_retry(job_id, task_id)
                .await
                .context("[BENTO-WF-111] Failed to update task retries through API")
        } else {
            self.task_db()?
                .update_task_retry(job_id, task_id)
                .await
                .context("[BENTO-WF-111] Failed to update task retries")
        }
    }

    async fn get_task_retries_running(
        &self,
        job_id: &uuid::Uuid,
        task_id: &str,
    ) -> Result<Option<i32>> {
        if uses_gpu_worker_api(&self.args.task_stream) {
            self.api_client
                .get_task_retries_running(job_id, task_id)
                .await
                .context("[BENTO-WF-109] Failed to read current retries through API")
        } else {
            self.task_db()?
                .get_task_retries_running(job_id, task_id)
                .await
                .context("[BENTO-WF-109] Failed to read current retries")
        }
    }

    pub(crate) async fn hot_get_bytes(&self, key: &str) -> Result<Vec<u8>> {
        if uses_gpu_worker_api(&self.args.task_stream) {
            self.api_client
                .hot_get_bytes(key)
                .await
                .with_context(|| format!("[BENTO-WF-202] Failed to fetch hot-store key {key}"))
        } else {
            let mut conn = self.redis_pool()?.get().await?;
            crate::redis::get_key(&mut conn, key)
                .await
                .with_context(|| format!("[BENTO-WF-202] Failed to fetch redis key {key}"))
        }
    }

    pub(crate) async fn hot_set_bytes(&self, key: &str, value: Vec<u8>) -> Result<()> {
        if uses_gpu_worker_api(&self.args.task_stream) {
            self.api_client
                .hot_set_bytes(key, value, Some(self.args.redis_ttl))
                .await
                .with_context(|| format!("[BENTO-WF-203] Failed to write hot-store key {key}"))
        } else {
            let mut conn = self.redis_pool()?.get().await?;
            crate::redis::set_key_with_expiry(&mut conn, key, value, Some(self.args.redis_ttl))
                .await
                .with_context(|| format!("[BENTO-WF-203] Failed to write redis key {key}"))?;
            Ok(())
        }
    }

    pub(crate) async fn hot_delete(&self, key: &str) -> Result<()> {
        if uses_gpu_worker_api(&self.args.task_stream) {
            self.api_client
                .hot_delete(key)
                .await
                .with_context(|| format!("[BENTO-WF-205] Failed to delete hot-store key {key}"))
        } else {
            let mut conn = self.redis_pool()?.get().await?;
            let _: () = redis::AsyncCommands::unlink(&mut conn, key)
                .await
                .with_context(|| format!("[BENTO-WF-205] Failed to delete redis key {key}"))?;
            Ok(())
        }
    }

    pub(crate) async fn write_asset_buf(&self, key: &str, value: Vec<u8>) -> Result<()> {
        self.api_client
            .write_asset_buf(key, value)
            .await
            .with_context(|| format!("[BENTO-WF-206] Failed to upload asset {key} through API"))
    }

    pub(crate) async fn write_asset<T>(&self, key: &str, value: &T) -> Result<()>
    where
        T: serde::Serialize,
    {
        self.api_client.write_asset(key, value).await.with_context(|| {
            format!("[BENTO-WF-207] Failed to upload serialized asset {key} through API")
        })
    }

    pub(crate) async fn write_asset_file(
        &self,
        key: &str,
        path: impl AsRef<std::path::Path>,
    ) -> Result<()> {
        self.api_client.write_asset_file(key, path).await.with_context(|| {
            format!("[BENTO-WF-208] Failed to stream asset file {key} through API")
        })
    }

    /// Create a signal hook to flip a boolean if its triggered
    ///
    /// Allows us to catch SIGTERM and exit any hard loop
    fn create_sig_monitor() -> Result<Arc<AtomicBool>> {
        let term = Arc::new(AtomicBool::new(false));
        signal_hook::flag::register(signal_hook::consts::SIGTERM, Arc::clone(&term))?;
        Ok(term)
    }

    /// Starts the work polling, runs until sig_hook triggers
    ///
    /// This function will poll for work and dispatch to the [Self::process_work] function until
    /// the process is terminated. It also handles retries / failures depending on the
    /// [Self::process_work] result
    pub async fn poll_work(&self) -> Result<()> {
        let term_sig =
            Self::create_sig_monitor().context("[BENTO-WF-103] Failed to create signal hook")?;

        // Enables task retry management background thread, good for 1-2 aux workers to run in the
        // cluster
        if self.args.monitor_requeue {
            let term_sig_copy = term_sig.clone();
            let task_db_copy = self
                .direct_task_db()
                .cloned()
                .context("[BENTO-WF-104] monitor_requeue requires direct taskdb access")?;
            let requeue_interval = self.args.requeue_poll_interval;

            tokio::spawn(async move {
                loop {
                    if let Err(e) = Self::poll_for_requeue(
                        term_sig_copy.clone(),
                        task_db_copy.clone(),
                        requeue_interval,
                    )
                    .await
                    {
                        tracing::error!("[BENTO-WF-104] Completed job cleanup failed: {:#}", e);
                        time::sleep(time::Duration::from_secs(requeue_interval)).await;
                    }
                }
            });
        }

        // Enable stuck task maintenance for aux workers
        if self.args.task_stream == AUX_WORK_TYPE {
            if !self.args.disable_stuck_task_cleanup {
                let term_sig_copy = term_sig.clone();
                let task_db_copy = self
                    .direct_task_db()
                    .cloned()
                    .context("[BENTO-WF-105] AUX maintenance requires direct taskdb access")?;
                let stuck_tasks_interval = self.args.stuck_tasks_poll_interval;
                tokio::spawn(async move {
                    loop {
                        if let Err(e) = Self::poll_for_stuck_tasks(
                            term_sig_copy.clone(),
                            task_db_copy.clone(),
                            stuck_tasks_interval,
                        )
                        .await
                        {
                            tracing::error!("[BENTO-WF-105] Stuck tasks cleanup failed: {:#}", e);
                            time::sleep(time::Duration::from_secs(60)).await;
                        }
                    }
                });
            }

            // Enable completed job cleanup for aux workers
            if !self.args.disable_completed_cleanup {
                let term_sig_copy = term_sig.clone();
                let task_db_copy = self
                    .direct_task_db()
                    .cloned()
                    .context("[BENTO-WF-106] AUX cleanup requires direct taskdb access")?;
                let cleanup_interval = self.args.cleanup_poll_interval;
                tokio::spawn(async move {
                    loop {
                        if let Err(e) = Self::poll_for_completed_job_cleanup(
                            term_sig_copy.clone(),
                            task_db_copy.clone(),
                            cleanup_interval,
                        )
                        .await
                        {
                            tracing::error!("[BENTO-WF-106] Completed job cleanup failed: {:#}", e);
                            time::sleep(time::Duration::from_secs(cleanup_interval)).await;
                        }
                    }
                });
            }
        }

        while !term_sig.load(Ordering::Relaxed) {
            // Update database pool metrics periodically
            self.update_db_pool_metrics();
            if let Err(err) = self.refresh_task_queue_metrics().await {
                tracing::warn!("Failed to refresh task queue metrics: {err:#}");
            }

            let task = match self.request_work().await {
                Ok(task) => {
                    let result = if task.is_some() { "claimed" } else { "empty" };
                    helpers::record_task_claim(&self.args.task_stream, result);
                    task
                }
                Err(err) => {
                    helpers::record_task_claim(&self.args.task_stream, "error");
                    return Err(err);
                }
            };
            let Some(task) = task else {
                continue;
            };

            let task_type = self.task_type_label(&task);
            let processing_start = Instant::now();
            let process_result = self.process_work(&task).await;
            let processing_status = if process_result.is_ok() { "success" } else { "error" };
            helpers::record_task_processing(
                &task_type,
                processing_status,
                processing_start.elapsed().as_secs_f64(),
            );

            if let Err(err) = process_result {
                let mut err_str = format!("{:#}", err);
                if !err_str.contains("stopped intentionally due to session limit")
                    && !err_str.contains("Session limit exceeded")
                {
                    tracing::error!("[BENTO-WF-108] Failure during task processing: {err_str}");
                }

                if task.max_retries > 0 {
                    // If the next retry would exceed the limit, set a final error now
                    if let Some(current_retries) =
                        self.get_task_retries_running(&task.job_id, &task.task_id).await?
                        && current_retries + 1 > task.max_retries
                    {
                        helpers::record_task_max_retries_exhausted(&task_type);
                        // Prevent massive errors from being reported to the DB
                        err_str.truncate(1024);
                        let final_err = if err_str.is_empty() {
                            "retry max hit".to_string()
                        } else {
                            format!("retry max hit: {}", err_str)
                        };
                        self.update_task_failed(&task.job_id, &task.task_id, &final_err).await?;
                        continue;
                    }

                    if self.update_task_retry(&task.job_id, &task.task_id).await? {
                        helpers::record_task_retry_attempt(&task_type);
                    } else {
                        helpers::record_task_max_retries_exhausted(&task_type);
                        tracing::info!("update_task_retried failed: {}", task.job_id);
                    }
                } else {
                    helpers::record_task_max_retries_exhausted(&task_type);
                    // Prevent massive errors from being reported to the DB
                    err_str.truncate(1024);
                    self.update_task_failed(&task.job_id, &task.task_id, &err_str).await?;
                }
                continue;
            }
        }
        tracing::warn!("Handled SIGTERM, shutting down...");

        Ok(())
    }

    /// Process a task and dispatch based on the task type
    pub async fn process_work(&self, task: &ReadyTask) -> Result<()> {
        let task_type: TaskType = serde_json::from_value(task.task_def.clone())
            .with_context(|| format!("Invalid task_def: {}:{}", task.job_id, task.task_id))?;

        // run the task
        let res = match task_type {
            TaskType::Executor(req) => serde_json::to_value(
                tasks::executor::executor(self, &task.job_id, &req)
                    .await
                    .context("[BENTO-WF-113] Executor failed")?,
            )
            .context("[BENTO-WF-114] Failed to serialize executor response")?,
            TaskType::Prove(req) => serde_json::to_value(
                tasks::prove::prover(self, &task.job_id, &task.task_id, &req)
                    .await
                    .context("[BENTO-WF-115] Prove failed")?,
            )
            .context("[BENTO-WF-116] Failed to serialize prove response")?,
            TaskType::Join(req) => {
                // Route to POVW or regular join based on agent POVW setting
                if self.is_povw_enabled() {
                    serde_json::to_value(
                        tasks::join_povw::join_povw(self, &task.job_id, &req)
                            .await
                            .context("[BENTO-WF-117] POVW join failed")?,
                    )
                    .context("[BENTO-WF-118] Failed to serialize POVW join response")?
                } else {
                    serde_json::to_value(
                        tasks::join::join(self, &task.job_id, &req)
                            .await
                            .context("[BENTO-WF-119] Join failed")?,
                    )
                    .context("[BENTO-WF-120] Failed to serialize join response")?
                }
            }
            TaskType::Resolve(req) => {
                // Route to POVW or regular resolve based on agent POVW setting
                if self.is_povw_enabled() {
                    serde_json::to_value(
                        tasks::resolve_povw::resolve_povw(self, &task.job_id, &req)
                            .await
                            .context("[BENTO-WF-121] POVW resolve failed")?,
                    )
                    .context("[BENTO-WF-122] Failed to serialize POVW resolve response")?
                } else {
                    serde_json::to_value(
                        tasks::resolve::resolver(self, &task.job_id, &req)
                            .await
                            .context("[BENTO-WF-123] Resolve failed")?,
                    )
                    .context("[BENTO-WF-124] Failed to serialize resolve response")?
                }
            }
            TaskType::Finalize => serde_json::to_value(
                tasks::finalize::finalize(self, &task.job_id)
                    .await
                    .context("[BENTO-WF-125] Finalize failed")?,
            )
            .context("[BENTO-WF-126] Failed to serialize finalize response")?,
            TaskType::Snark(req) => serde_json::to_value(
                tasks::snark::stark2snark(self, &task.job_id.to_string(), &req)
                    .await
                    .context("[BENTO-WF-127] Snark failed")?,
            )
            .context("[BENTO-WF-128] failed to serialize snark response")?,
            TaskType::Keccak(req) => serde_json::to_value(
                tasks::keccak::keccak(self, &task.job_id, &task.task_id, &req)
                    .await
                    .context("[BENTO-WF-129] Keccak failed")?,
            )
            .context("[BENTO-WF-130] failed to serialize keccak response")?,
            TaskType::Union(req) => serde_json::to_value(
                tasks::union::union(self, &task.job_id, &req)
                    .await
                    .context("[BENTO-WF-131] Union failed")?,
            )
            .context("[BENTO-WF-132] failed to serialize union response")?,
        };

        self.update_task_done(&task.job_id, &task.task_id, res).await?;

        Ok(())
    }

    /// background task to poll for jobs that need to be requeued
    ///
    /// Scan the queue looking for tasks that need to be retried and update them
    /// the agent will catch and fail max retries.
    async fn poll_for_requeue(
        term_sig: Arc<AtomicBool>,
        task_db: TaskDb,
        poll_interval: u64,
    ) -> Result<()> {
        while !term_sig.load(Ordering::Relaxed) {
            tracing::debug!("Triggering a requeue job...");
            let requeued = task_db.requeue_tasks(100).await?;
            if requeued > 0 {
                helpers::record_task_requeues("all", requeued as u64);
                tracing::info!("Requeued {requeued} expired tasks for retry");
            }
            time::sleep(tokio::time::Duration::from_secs(poll_interval)).await;
        }

        Ok(())
    }

    /// background task to poll for stuck pending tasks and fix them
    ///
    /// Check for tasks that are stuck in pending state but should be ready
    /// because all their dependencies are complete, and fix them.
    async fn poll_for_stuck_tasks(
        term_sig: Arc<AtomicBool>,
        task_db: TaskDb,
        poll_interval: u64,
    ) -> Result<()> {
        while !term_sig.load(Ordering::Relaxed) {
            // Sleep before each check to avoid running on startup
            time::sleep(tokio::time::Duration::from_secs(poll_interval)).await;

            tracing::debug!("Checking for stuck pending tasks...");

            let stuck_tasks = task_db.check_stuck_pending_tasks().await?;
            let mut stuck_by_stream = std::collections::BTreeMap::<String, u64>::new();
            for task in &stuck_tasks {
                *stuck_by_stream.entry(task.worker_type.clone()).or_default() += 1;
            }
            helpers::reset_task_stuck_pending_current();
            for (task_stream, count) in &stuck_by_stream {
                helpers::set_task_stuck_pending_current(task_stream, *count as i64);
                helpers::record_task_stuck_pending(task_stream, *count);
            }
            if !stuck_tasks.is_empty() {
                tracing::error!(
                    count = stuck_tasks.len(),
                    "STUCK TASKS DETECTED — pending tasks with all prerequisites done (possible data inconsistency)"
                );
                for task in &stuck_tasks {
                    tracing::error!(
                        job_id = %task.job_id,
                        task_id = %task.task_id,
                        task_stream = %task.worker_type,
                        waiting_on = task.waiting_on,
                        actual_deps = task.actual_deps,
                        completed_deps = task.completed_deps,
                        "Stuck task details"
                    );
                }
            }
        }

        Ok(())
    }

    /// background task to clean up completed jobs
    ///
    /// Remove completed jobs and their associated tasks and dependencies
    /// to prevent database bloat over time.
    async fn poll_for_completed_job_cleanup(
        term_sig: Arc<AtomicBool>,
        task_db: TaskDb,
        poll_interval: u64,
    ) -> Result<()> {
        while !term_sig.load(Ordering::Relaxed) {
            // Sleep before each check to avoid running on startup
            time::sleep(tokio::time::Duration::from_secs(poll_interval)).await;

            tracing::debug!("Cleaning up completed jobs...");

            let cleared_count = task_db.clear_completed_jobs().await?;
            if cleared_count > 0 {
                tracing::info!("Cleared {} completed jobs", cleared_count);
                workflow_common::metrics::helpers::record_completed_jobs_garbage_collection_metrics(
                    cleared_count as u64,
                );
            }
        }

        Ok(())
    }
}
