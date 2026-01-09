// Copyright 2026 Boundless Foundation, Inc.
//
// Use of this source code is governed by the Business Source License
// as found in the LICENSE-BSL file.

#![deny(missing_docs)]

//! Workflow processing Agent service

use crate::redis::RedisPool;
use anyhow::{Context, Result};
use clap::Parser;
use risc0_zkvm::{ProverOpts, ProverServer, VerifierContext, get_prover_server};
use sqlx::postgres::{PgPool, PgPoolOptions};
use std::{
    rc::Rc,
    str::FromStr,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};
use taskdb::ReadyTask;
use tokio::time;
use workflow_common::{COPROC_WORK_TYPE, TaskType};
mod redis;
mod tasks;

pub use workflow_common::{
    AUX_WORK_TYPE, EXEC_WORK_TYPE, JOIN_WORK_TYPE, PROVE_WORK_TYPE, SNARK_RETRIES_DEFAULT,
    SNARK_TIMEOUT_DEFAULT, s3::S3Client,
};

/// Workflow agent
///
/// Monitors taskdb for new tasks on the selected stream and processes the work.
/// Requires redis / task (psql) access
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

    /// taskdb postgres DATABASE_URL
    #[clap(env)]
    pub database_url: String,

    /// redis connection URL
    #[clap(env)]
    pub redis_url: String,

    /// risc0 segment po2 arg
    #[clap(env, short, long, default_value_t = 20)]
    pub segment_po2: u32,

    /// max connections to SQL db in connection pool
    #[clap(env, long, default_value_t = 1)]
    pub db_max_connections: u32,

    /// Redis TTL, seconds before objects expire automatically
    ///
    /// Defaults to 8 hours
    #[clap(env,long, default_value_t = 8 * 60 * 60)]
    pub redis_ttl: u64,

    /// Executor limit, in millions of cycles
    #[clap(env, short, long, default_value_t = 100_000)]
    pub exec_cycle_limit: u64,

    /// S3 / Minio bucket
    #[clap(env)]
    pub s3_bucket: String,

    /// S3 / Minio access key
    #[clap(env)]
    pub s3_access_key: String,

    /// S3 / Minio secret key
    #[clap(env)]
    pub s3_secret_key: String,

    /// S3 / Minio url
    #[clap(env)]
    pub s3_url: String,

    /// S3 region, can be anything if using minio
    #[clap(env, default_value = "us-west-2")]
    pub s3_region: String,

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
    #[clap(env, long, default_value_t = 60 * 60)]
    cleanup_poll_interval: u64,

    /// Disable cron to clean up completed jobs in taskdb.
    #[clap(long, default_value_t = true, env = "BENTO_DISABLE_COMPLETED_CLEANUP")]
    disable_completed_cleanup: bool,

    /// Disable cron to clean up stuck tasks in taskdb.
    #[clap(long, env = "BENTO_DISABLE_STUCK_TASK_CLEANUP")]
    disable_stuck_task_cleanup: bool,
}

/// Core agent context to hold all optional clients / pools and state
pub struct Agent {
    /// Postgresql database connection pool
    pub db_pool: PgPool,
    /// segment po2 config
    pub segment_po2: u32,
    /// redis connection pool
    pub redis_pool: RedisPool,
    /// S3 client
    pub s3_client: S3Client,
    /// all configuration params:
    args: Args,
    /// risc0 Prover server
    prover: Option<Rc<dyn ProverServer>>,
    /// risc0 verifier context
    verifier_ctx: VerifierContext,
}

impl Agent {
    /// Check if POVW is enabled for this agent instance
    pub fn is_povw_enabled(&self) -> bool {
        std::env::var("POVW_LOG_ID").is_ok()
    }

    /// Update database connection pool metrics
    pub fn update_db_pool_metrics(&self) {
        use workflow_common::metrics::helpers;
        let size = self.db_pool.size() as i64;
        let idle = self.db_pool.num_idle() as i64;
        let active = size - idle;
        helpers::update_db_pool_metrics(size, idle, active);
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

        let db_pool = PgPoolOptions::new()
            .max_connections(args.db_max_connections)
            .connect(&args.database_url)
            .await
            .context("[BENTO-WF-100] Failed to initialize postgresql pool")?;
        let redis_pool = crate::redis::create_pool(&args.redis_url)?;
        let s3_client = S3Client::from_minio(
            &args.s3_url,
            &args.s3_bucket,
            &args.s3_access_key,
            &args.s3_secret_key,
            &args.s3_region,
        )
        .await
        .context("[BENTO-WF-101] Failed to initialize s3 client / bucket")?;

        let verifier_ctx = VerifierContext::default();
        let prover = if args.task_stream == PROVE_WORK_TYPE
            || args.task_stream == JOIN_WORK_TYPE
            || args.task_stream == COPROC_WORK_TYPE
        {
            let opts = ProverOpts::default();
            let prover = get_prover_server(&opts)
                .context("[BENTO-WF-102] Failed to initialize prover server")?;
            Some(prover)
        } else {
            None
        };

        Ok(Self {
            db_pool,
            segment_po2: args.segment_po2,
            redis_pool,
            s3_client,
            args,
            prover,
            verifier_ctx,
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
            let db_pool_copy = self.db_pool.clone();
            let requeue_interval = self.args.requeue_poll_interval;

            tokio::spawn(async move {
                loop {
                    if let Err(e) = Self::poll_for_requeue(
                        term_sig_copy.clone(),
                        db_pool_copy.clone(),
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
                let db_pool_copy = self.db_pool.clone();
                let stuck_tasks_interval = self.args.stuck_tasks_poll_interval;
                tokio::spawn(async move {
                    loop {
                        if let Err(e) = Self::poll_for_stuck_tasks(
                            term_sig_copy.clone(),
                            db_pool_copy.clone(),
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
                let db_pool_copy = self.db_pool.clone();
                let cleanup_interval = self.args.cleanup_poll_interval;
                tokio::spawn(async move {
                    loop {
                        if let Err(e) = Self::poll_for_completed_job_cleanup(
                            term_sig_copy.clone(),
                            db_pool_copy.clone(),
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

            let task = taskdb::request_work(&self.db_pool, &self.args.task_stream)
                .await
                .context("[BENTO-WF-107] Failed to request_work")?;
            let Some(task) = task else {
                time::sleep(time::Duration::from_secs(self.args.poll_time)).await;
                continue;
            };

            if let Err(err) = self.process_work(&task).await {
                let mut err_str = format!("{:#}", err);
                if !err_str.contains("stopped intentionally due to session limit")
                    && !err_str.contains("Session limit exceeded")
                {
                    tracing::error!("[BENTO-WF-108] Failure during task processing: {err_str}");
                }

                if task.max_retries > 0 {
                    // If the next retry would exceed the limit, set a final error now
                    if let Some(current_retries) = sqlx::query_scalar::<_, i32>(
                        "SELECT retries FROM tasks WHERE job_id = $1 AND task_id = $2 AND state = 'running'",
                    )
                    .bind(task.job_id)
                    .bind(&task.task_id)
                    .fetch_optional(&self.db_pool)
                    .await
                    .context("[BENTO-WF-109] Failed to read current retries")?
                        && current_retries + 1 > task.max_retries {
                            // Prevent massive errors from being reported to the DB
                            err_str.truncate(1024);
                            let final_err = if err_str.is_empty() {
                                "retry max hit".to_string()
                            } else {
                                format!("retry max hit: {}", err_str)
                            };
                            taskdb::update_task_failed(
                                &self.db_pool,
                                &task.job_id,
                                &task.task_id,
                                &final_err,
                            )
                            .await
                            .context("[BENTO-WF-110] Failed to report task failure")?;
                            continue;
                        }

                    if !taskdb::update_task_retry(&self.db_pool, &task.job_id, &task.task_id)
                        .await
                        .context("[BENTO-WF-111] Failed to update task retries")?
                    {
                        tracing::info!("update_task_retried failed: {}", task.job_id);
                    }
                } else {
                    // Prevent massive errors from being reported to the DB
                    err_str.truncate(1024);
                    taskdb::update_task_failed(
                        &self.db_pool,
                        &task.job_id,
                        &task.task_id,
                        &err_str,
                    )
                    .await
                    .context("[BENTO-WF-112] Failed to report task failure")?;
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
            TaskType::Finalize(req) => serde_json::to_value(
                tasks::finalize::finalize(self, &task.job_id, &req)
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

        taskdb::update_task_done(&self.db_pool, &task.job_id, &task.task_id, res)
            .await
            .context("[BENTO-WF-133] Failed to report task done")?;

        Ok(())
    }

    /// background task to poll for jobs that need to be requeued
    ///
    /// Scan the queue looking for tasks that need to be retried and update them
    /// the agent will catch and fail max retries.
    async fn poll_for_requeue(
        term_sig: Arc<AtomicBool>,
        db_pool: PgPool,
        poll_interval: u64,
    ) -> Result<()> {
        while !term_sig.load(Ordering::Relaxed) {
            tracing::debug!("Triggering a requeue job...");
            let retry_tasks = taskdb::requeue_tasks(&db_pool, 100).await?;
            if retry_tasks > 0 {
                tracing::info!("Found {retry_tasks} tasks that needed to be retried");
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
        db_pool: PgPool,
        poll_interval: u64,
    ) -> Result<()> {
        while !term_sig.load(Ordering::Relaxed) {
            // Sleep before each check to avoid running on startup
            time::sleep(tokio::time::Duration::from_secs(poll_interval)).await;

            tracing::debug!("Checking for stuck pending tasks...");

            // First check if there are any stuck tasks
            let stuck_tasks = taskdb::check_stuck_pending_tasks(&db_pool).await?;
            if !stuck_tasks.is_empty() {
                tracing::warn!("Found {} stuck pending tasks", stuck_tasks.len());

                // Log details about stuck tasks
                for task in &stuck_tasks {
                    tracing::info!(
                        "Stuck task: job_id={}, task_id={}, waiting_on={}, actual_deps={}, completed_deps={}",
                        task.job_id,
                        task.task_id,
                        task.waiting_on,
                        task.actual_deps,
                        task.completed_deps
                    );
                }

                // Fix the stuck tasks
                let fixed_count = taskdb::fix_stuck_pending_tasks(&db_pool).await?;
                if fixed_count > 0 {
                    tracing::info!("Fixed {} stuck pending tasks", fixed_count);
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
        db_pool: PgPool,
        poll_interval: u64,
    ) -> Result<()> {
        while !term_sig.load(Ordering::Relaxed) {
            // Sleep before each check to avoid running on startup
            time::sleep(tokio::time::Duration::from_secs(poll_interval)).await;

            tracing::debug!("Cleaning up completed jobs...");

            let cleared_count = taskdb::clear_completed_jobs(&db_pool).await?;
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
