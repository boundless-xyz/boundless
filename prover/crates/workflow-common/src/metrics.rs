// Copyright 2026 Boundless Foundation, Inc.
//
// Use of this source code is governed by the Business Source License
// as found in the LICENSE-BSL file.

use lazy_static::lazy_static;
use prometheus::core::Collector;
use prometheus::{
    Histogram, HistogramOpts, HistogramVec, IntCounter, IntCounterVec, IntGauge, Opts,
};

// Prometheus metrics for workflow execution
lazy_static! {
    // Execution metrics
    pub static ref EXECUTION_DURATION: HistogramVec = HistogramVec::new(
        HistogramOpts::new("execution_duration_seconds", "Duration of job execution in seconds")
            .buckets(vec![0.1, 0.5, 1.0, 2.5, 5.0, 10.0, 25.0, 50.0, 100.0, 250.0, 500.0, 1000.0]),
        &["job_type", "status"]
    ).unwrap();

    pub static ref SEGMENT_COUNT: IntCounter = IntCounter::new(
        "segments_total", "Total number of segments processed"
    ).unwrap();

    pub static ref USER_CYCLES: IntCounter = IntCounter::new(
        "user_cycles_total", "Total user cycles executed"
    ).unwrap();

    pub static ref TOTAL_CYCLES: IntCounter = IntCounter::new(
        "total_cycles_total", "Total cycles executed"
    ).unwrap();

    // Task processing metrics
    pub static ref TASKS_CREATED: IntCounterVec = IntCounterVec::new(
        Opts::new("tasks_created_total", "Total number of tasks created by type"),
        &["task_type"]
    ).unwrap();

    pub static ref TASK_PROCESSING_DURATION: HistogramVec = HistogramVec::new(
        HistogramOpts::new("task_processing_duration_seconds", "Duration of task processing")
            .buckets(vec![0.001, 0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0]),
        &["task_type", "status"]
    ).unwrap();

    // Error metrics
    pub static ref EXECUTION_ERRORS: IntCounterVec = IntCounterVec::new(
        Opts::new("errors_total", "Total number of execution errors by type"),
        &["error_type"]
    ).unwrap();

    pub static ref GUEST_FAULTS: IntCounter = IntCounter::new(
        "guest_faults_total", "Total number of guest faults"
    ).unwrap();

    // I/O metrics
    pub static ref S3_OPERATIONS: IntCounterVec = IntCounterVec::new(
        Opts::new("s3_operations_total", "Total number of S3 operations by type"),
        &["operation_type", "status"]
    ).unwrap();

    pub static ref S3_OPERATION_DURATION: HistogramVec = HistogramVec::new(
        HistogramOpts::new("s3_operation_duration_seconds", "Duration of S3 operations")
            .buckets(vec![0.01, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0, 25.0, 50.0]),
        &["operation_type", "status"]
    ).unwrap();

    pub static ref REDIS_OPERATIONS: IntCounterVec = IntCounterVec::new(
        Opts::new("redis_operations_total", "Total number of Redis operations by type"),
        &["operation_type", "status"]
    ).unwrap();

    pub static ref REDIS_OPERATION_DURATION: HistogramVec = HistogramVec::new(
        HistogramOpts::new("redis_operation_duration_seconds", "Duration of Redis operations")
            .buckets(vec![0.001, 0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0]),
        &["operation_type", "status"]
    ).unwrap();

    // Database operation metrics
    pub static ref DB_OPERATIONS: IntCounterVec = IntCounterVec::new(
        Opts::new("db_operations_total", "Total number of database operations by type"),
        &["operation_type", "status"]
    ).unwrap();

    pub static ref DB_OPERATION_DURATION: HistogramVec = HistogramVec::new(
        HistogramOpts::new("db_operation_duration_seconds", "Duration of database operations")
            .buckets(vec![0.001, 0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0]),
        &["operation_type", "status"]
    ).unwrap();

    pub static ref DB_CONNECTION_POOL_SIZE: IntGauge = IntGauge::new(
        "db_connection_pool_size", "Current database connection pool size"
    ).unwrap();

    pub static ref DB_CONNECTION_POOL_IDLE: IntGauge = IntGauge::new(
        "db_connection_pool_idle", "Current number of idle database connections"
    ).unwrap();

    pub static ref DB_CONNECTION_POOL_ACTIVE: IntGauge = IntGauge::new(
        "db_connection_pool_active", "Current number of active database connections"
    ).unwrap();

    // Queue metrics
    pub static ref SEGMENT_QUEUE_SIZE: IntGauge = IntGauge::new(
        "segment_queue_size", "Current size of segment queue"
    ).unwrap();

    pub static ref TASK_QUEUE_SIZE_GAUGE: IntGauge = IntGauge::new(
        "task_queue_size", "Current size of task queue"
    ).unwrap();

    // Assumption metrics
    pub static ref ASSUMPTION_COUNT: IntCounter = IntCounter::new(
        "assumptions_total", "Total number of assumptions processed"
    ).unwrap();

    pub static ref ASSUMPTION_PROCESSING_DURATION: HistogramVec = HistogramVec::new(
        HistogramOpts::new("assumption_processing_duration_seconds", "Duration of assumption processing")
            .buckets(vec![0.01, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0]),
        &["assumption_type", "status"]
    ).unwrap();

    // Resolve POVW specific metrics
    pub static ref POVW_RESOLVE_DURATION: HistogramVec = HistogramVec::new(
        HistogramOpts::new("povw_resolve_duration_seconds", "Duration of POVW resolve operations")
            .buckets(vec![0.1, 0.5, 1.0, 2.5, 5.0, 10.0, 25.0, 50.0, 100.0]),
        &["operation_type", "status"]
    ).unwrap();

    pub static ref POVW_RESOLVE_OPERATIONS: IntCounterVec = IntCounterVec::new(
        Opts::new("povw_resolve_operations_total", "Total number of POVW resolve operations by type"),
        &["operation_type", "status"]
    ).unwrap();

    // General task metrics
    pub static ref TASK_DURATION: HistogramVec = HistogramVec::new(
        HistogramOpts::new("task_duration_seconds", "Duration of task execution")
            .buckets(vec![0.1, 0.5, 1.0, 2.5, 5.0, 10.0, 25.0, 50.0, 100.0, 250.0, 500.0]),
        &["task_name", "operation_type", "status"]
    ).unwrap();

    pub static ref TASK_OPERATIONS: IntCounterVec = IntCounterVec::new(
        Opts::new("task_operations_total", "Total number of task operations by type and status"),
        &["task_name", "operation_type", "status"]
    ).unwrap();
    pub static ref COMPLETED_JOBS_METRICS: IntCounterVec = IntCounterVec::new(
        Opts::new("completed_jobs_total", "Total number of completed jobs by type"),
        &["job_type"]
    ).unwrap();
}

/// Helper functions for common metric operations
pub mod helpers {
    use super::*;
    use anyhow::Result;
    use std::net::SocketAddr;
    use std::sync::Mutex;
    use std::time::Instant;

    lazy_static! {
        static ref METRICS_EXPORTER: Mutex<Option<prometheus_exporter::Exporter>> =
            Mutex::new(None);
    }

    fn register_or_log<C>(metric_name: &str, collector: &C)
    where
        C: Collector + Clone + 'static,
    {
        if let Err(e) = register_collector(collector) {
            tracing::error!("Failed to register {metric_name} metrics: {e:?}");
        }
    }

    fn register_static_metrics() {
        register_or_log("execution_duration_seconds", &*EXECUTION_DURATION);
        register_or_log("segments_total", &*SEGMENT_COUNT);
        register_or_log("user_cycles_total", &*USER_CYCLES);
        register_or_log("total_cycles_total", &*TOTAL_CYCLES);
        register_or_log("tasks_created_total", &*TASKS_CREATED);
        register_or_log("task_processing_duration_seconds", &*TASK_PROCESSING_DURATION);
        register_or_log("errors_total", &*EXECUTION_ERRORS);
        register_or_log("guest_faults_total", &*GUEST_FAULTS);
        register_or_log("s3_operations_total", &*S3_OPERATIONS);
        register_or_log("s3_operation_duration_seconds", &*S3_OPERATION_DURATION);
        register_or_log("redis_operations_total", &*REDIS_OPERATIONS);
        register_or_log("redis_operation_duration_seconds", &*REDIS_OPERATION_DURATION);
        register_or_log("db_operations_total", &*DB_OPERATIONS);
        register_or_log("db_operation_duration_seconds", &*DB_OPERATION_DURATION);
        register_or_log("db_connection_pool_size", &*DB_CONNECTION_POOL_SIZE);
        register_or_log("db_connection_pool_idle", &*DB_CONNECTION_POOL_IDLE);
        register_or_log("db_connection_pool_active", &*DB_CONNECTION_POOL_ACTIVE);
        register_or_log("segment_queue_size", &*SEGMENT_QUEUE_SIZE);
        register_or_log("task_queue_size", &*TASK_QUEUE_SIZE_GAUGE);
        register_or_log("assumptions_total", &*ASSUMPTION_COUNT);
        register_or_log("assumption_processing_duration_seconds", &*ASSUMPTION_PROCESSING_DURATION);
        register_or_log("povw_resolve_duration_seconds", &*POVW_RESOLVE_DURATION);
        register_or_log("povw_resolve_operations_total", &*POVW_RESOLVE_OPERATIONS);
        register_or_log("task_duration_seconds", &*TASK_DURATION);
        register_or_log("task_operations_total", &*TASK_OPERATIONS);
        register_or_log("completed_jobs_total", &*COMPLETED_JOBS_METRICS);
    }

    /// Register all metrics with the default Prometheus registry.
    /// This function is used to register the metrics with the default Prometheus registry.
    pub fn register_collector<C>(collector: &C) -> std::result::Result<(), prometheus::Error>
    where
        C: Collector + Clone + 'static,
    {
        prometheus::default_registry().register(Box::new(collector.clone())).or_else(
            |err| match err {
                prometheus::Error::AlreadyReg { .. } => Ok(()),
                other => Err(other),
            },
        )
    }

    pub fn start_metrics_exporter() -> Result<()> {
        let mut exporter_guard = METRICS_EXPORTER
            .lock()
            .map_err(|e| anyhow::anyhow!("Failed to lock metrics exporter guard: {e}"))?;

        if exporter_guard.is_some() {
            return Ok(());
        }

        let metrics_addr = std::env::var("PROMETHEUS_METRICS_ADDR")
            .unwrap_or_else(|_| "0.0.0.0:9090".to_string())
            .parse::<SocketAddr>()
            .unwrap_or_else(|_| SocketAddr::from(([0, 0, 0, 0], 9090)));

        let exporter = prometheus_exporter::start(metrics_addr)
            .map_err(|e| anyhow::anyhow!("Failed to start metrics exporter: {e:?}"))?;
        register_static_metrics();
        *exporter_guard = Some(exporter);
        Ok(())
    }

    /// Record the duration of an operation with error handling
    pub fn record_operation_duration<F, T, E>(histogram: &Histogram, operation: F) -> Result<T, E>
    where
        F: FnOnce() -> Result<T, E>,
    {
        let start = Instant::now();
        let result = operation();
        histogram.observe(start.elapsed().as_secs_f64());
        result
    }

    /// Execute an async operation with automatic S3 metrics recording.
    /// Uses a tracing span for automatic duration tracking.
    pub async fn with_s3_metrics<F, Fut, T>(operation_type: &str, operation: F) -> anyhow::Result<T>
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = anyhow::Result<T>>,
    {
        let span =
            tracing::span!(tracing::Level::DEBUG, "s3_operation", operation = operation_type);
        let _guard = span.entered();
        let start = Instant::now();
        let result = operation().await;
        let duration = start.elapsed().as_secs_f64();
        let status = if result.is_ok() { "success" } else { "error" };
        record_s3_operation(operation_type, status, duration);
        result
    }

    pub async fn with_redis_metrics<F, Fut, T, E>(
        operation_type: &str,
        operation: F,
    ) -> std::result::Result<T, E>
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = std::result::Result<T, E>>,
    {
        let span =
            tracing::span!(tracing::Level::DEBUG, "redis_operation", operation = operation_type);
        let _guard = span.entered();
        let start = Instant::now();
        let result = operation().await;
        let duration = start.elapsed().as_secs_f64();
        let status = if result.is_ok() { "success" } else { "error" };
        record_redis_operation(operation_type, status, duration);
        result
    }

    pub async fn with_task_metrics<F, Fut, T>(
        task_name: &str,
        operation_type: &str,
        operation: F,
    ) -> anyhow::Result<T>
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = anyhow::Result<T>>,
    {
        let span = tracing::span!(
            tracing::Level::DEBUG,
            "task_operation",
            task = task_name,
            operation = operation_type
        );
        let _guard = span.entered();
        let start = Instant::now();
        let result = operation().await;
        let duration = start.elapsed().as_secs_f64();
        let status = if result.is_ok() { "success" } else { "error" };
        record_task_operation(task_name, operation_type, status, duration);
        result
    }

    /// Record S3 operation metrics
    pub fn record_s3_operation(operation_type: &str, status: &str, duration_seconds: f64) {
        register_or_log("s3_operations_total", &*S3_OPERATIONS);
        register_or_log("s3_operation_duration_seconds", &*S3_OPERATION_DURATION);
        S3_OPERATIONS.with_label_values(&[operation_type, status]).inc();
        S3_OPERATION_DURATION
            .with_label_values(&[operation_type, status])
            .observe(duration_seconds);
    }

    /// Record Redis operation metrics
    pub fn record_redis_operation(operation_type: &str, status: &str, duration_seconds: f64) {
        register_or_log("redis_operations_total", &*REDIS_OPERATIONS);
        register_or_log("redis_operation_duration_seconds", &*REDIS_OPERATION_DURATION);
        REDIS_OPERATIONS.with_label_values(&[operation_type, status]).inc();
        REDIS_OPERATION_DURATION
            .with_label_values(&[operation_type, status])
            .observe(duration_seconds);
    }

    /// Record task operation metrics
    pub fn record_task(task_name: &str, operation_type: &str, status: &str, duration_seconds: f64) {
        record_task_operation(task_name, operation_type, status, duration_seconds);
    }

    /// Record database operation metrics
    pub fn record_db_operation(operation_type: &str, status: &str, duration_seconds: f64) {
        register_or_log("db_operations_total", &*DB_OPERATIONS);
        register_or_log("db_operation_duration_seconds", &*DB_OPERATION_DURATION);
        DB_OPERATIONS.with_label_values(&[operation_type, status]).inc();
        DB_OPERATION_DURATION
            .with_label_values(&[operation_type, status])
            .observe(duration_seconds);
    }

    /// Update database connection pool metrics
    pub fn update_db_pool_metrics(size: i64, idle: i64, active: i64) {
        register_or_log("db_connection_pool_size", &*DB_CONNECTION_POOL_SIZE);
        register_or_log("db_connection_pool_idle", &*DB_CONNECTION_POOL_IDLE);
        register_or_log("db_connection_pool_active", &*DB_CONNECTION_POOL_ACTIVE);
        DB_CONNECTION_POOL_SIZE.set(size);
        DB_CONNECTION_POOL_IDLE.set(idle);
        DB_CONNECTION_POOL_ACTIVE.set(active);
    }

    /// Record task operation metrics (consolidated function for all task operations)
    pub fn record_task_operation(
        task_name: &str,
        operation_type: &str,
        status: &str,
        duration_seconds: f64,
    ) {
        register_or_log("task_operations_total", &*TASK_OPERATIONS);
        register_or_log("task_duration_seconds", &*TASK_DURATION);
        TASK_OPERATIONS.with_label_values(&[task_name, operation_type, status]).inc();
        TASK_DURATION
            .with_label_values(&[task_name, operation_type, status])
            .observe(duration_seconds);
    }

    /// Record execution duration
    pub fn record_execution_duration(job_type: &str, status: &str, duration_seconds: f64) {
        register_or_log("execution_duration_seconds", &*EXECUTION_DURATION);
        EXECUTION_DURATION.with_label_values(&[job_type, status]).observe(duration_seconds);
    }

    /// Record assumption processing duration
    pub fn record_assumption_duration(assumption_type: &str, status: &str, duration_seconds: f64) {
        register_or_log("assumption_processing_duration_seconds", &*ASSUMPTION_PROCESSING_DURATION);
        ASSUMPTION_PROCESSING_DURATION
            .with_label_values(&[assumption_type, status])
            .observe(duration_seconds);
    }
    /// Record completed jobs metrics
    pub fn record_completed_jobs_garbage_collection_metrics(count: u64) {
        register_or_log("completed_jobs_total", &*COMPLETED_JOBS_METRICS);
        COMPLETED_JOBS_METRICS.with_label_values(&["garbage_collection"]).inc_by(count);
    }
}
