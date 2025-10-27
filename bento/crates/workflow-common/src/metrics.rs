// Copyright 2025 RISC Zero, Inc.
//
// Use of this source code is governed by the Business Source License
// as found in the LICENSE-BSL file.

use lazy_static::lazy_static;
use prometheus::{Histogram, HistogramOpts, IntCounter, IntCounterVec, IntGauge, Opts};

// Prometheus metrics for workflow execution
lazy_static! {
    // Execution metrics
    pub static ref EXECUTION_DURATION: Histogram = Histogram::with_opts(
        HistogramOpts::new("executor_execution_duration_seconds", "Duration of job execution in seconds")
            .buckets(vec![0.1, 0.5, 1.0, 2.5, 5.0, 10.0, 25.0, 50.0, 100.0, 250.0, 500.0, 1000.0])
    ).unwrap();

    pub static ref SEGMENT_COUNT: IntCounter = IntCounter::new(
        "executor_segments_total", "Total number of segments processed"
    ).unwrap();

    pub static ref USER_CYCLES: IntCounter = IntCounter::new(
        "executor_user_cycles_total", "Total user cycles executed"
    ).unwrap();

    pub static ref TOTAL_CYCLES: IntCounter = IntCounter::new(
        "executor_total_cycles_total", "Total cycles executed"
    ).unwrap();

    // Task processing metrics
    pub static ref TASKS_CREATED: IntCounterVec = IntCounterVec::new(
        Opts::new("executor_tasks_created_total", "Total number of tasks created by type"),
        &["task_type"]
    ).unwrap();

    pub static ref TASK_PROCESSING_DURATION: Histogram = Histogram::with_opts(
        HistogramOpts::new("executor_task_processing_duration_seconds", "Duration of task processing")
            .buckets(vec![0.001, 0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0])
    ).unwrap();

    // Error metrics
    pub static ref EXECUTION_ERRORS: IntCounterVec = IntCounterVec::new(
        Opts::new("executor_errors_total", "Total number of execution errors by type"),
        &["error_type"]
    ).unwrap();

    pub static ref GUEST_FAULTS: IntCounter = IntCounter::new(
        "executor_guest_faults_total", "Total number of guest faults"
    ).unwrap();

    // I/O metrics
    pub static ref S3_OPERATIONS: IntCounterVec = IntCounterVec::new(
        Opts::new("executor_s3_operations_total", "Total number of S3 operations by type"),
        &["operation_type", "status"]
    ).unwrap();

    pub static ref S3_OPERATION_DURATION: Histogram = Histogram::with_opts(
        HistogramOpts::new("executor_s3_operation_duration_seconds", "Duration of S3 operations")
            .buckets(vec![0.01, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0, 25.0, 50.0])
    ).unwrap();

    pub static ref REDIS_OPERATIONS: IntCounterVec = IntCounterVec::new(
        Opts::new("executor_redis_operations_total", "Total number of Redis operations by type"),
        &["operation_type", "status"]
    ).unwrap();

    pub static ref REDIS_OPERATION_DURATION: Histogram = Histogram::with_opts(
        HistogramOpts::new("executor_redis_operation_duration_seconds", "Duration of Redis operations")
            .buckets(vec![0.001, 0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0])
    ).unwrap();

    // Database operation metrics
    pub static ref DB_OPERATIONS: IntCounterVec = IntCounterVec::new(
        Opts::new("executor_db_operations_total", "Total number of database operations by type"),
        &["operation_type", "status"]
    ).unwrap();

    pub static ref DB_OPERATION_DURATION: Histogram = Histogram::with_opts(
        HistogramOpts::new("executor_db_operation_duration_seconds", "Duration of database operations")
            .buckets(vec![0.001, 0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0])
    ).unwrap();

    pub static ref DB_CONNECTION_POOL_SIZE: IntGauge = IntGauge::new(
        "executor_db_connection_pool_size", "Current database connection pool size"
    ).unwrap();

    pub static ref DB_CONNECTION_POOL_IDLE: IntGauge = IntGauge::new(
        "executor_db_connection_pool_idle", "Current number of idle database connections"
    ).unwrap();

    pub static ref DB_CONNECTION_POOL_ACTIVE: IntGauge = IntGauge::new(
        "executor_db_connection_pool_active", "Current number of active database connections"
    ).unwrap();

    // Queue metrics
    pub static ref SEGMENT_QUEUE_SIZE: IntGauge = IntGauge::new(
        "executor_segment_queue_size", "Current size of segment queue"
    ).unwrap();

    pub static ref TASK_QUEUE_SIZE_GAUGE: IntGauge = IntGauge::new(
        "executor_task_queue_size", "Current size of task queue"
    ).unwrap();

    // Assumption metrics
    pub static ref ASSUMPTION_COUNT: IntCounter = IntCounter::new(
        "executor_assumptions_total", "Total number of assumptions processed"
    ).unwrap();

    pub static ref ASSUMPTION_PROCESSING_DURATION: Histogram = Histogram::with_opts(
        HistogramOpts::new("executor_assumption_processing_duration_seconds", "Duration of assumption processing")
            .buckets(vec![0.01, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0])
    ).unwrap();

    // Resolve POVW specific metrics
    pub static ref POVW_RESOLVE_DURATION: Histogram = Histogram::with_opts(
        HistogramOpts::new("povw_resolve_duration_seconds", "Duration of POVW resolve operations")
            .buckets(vec![0.1, 0.5, 1.0, 2.5, 5.0, 10.0, 25.0, 50.0, 100.0])
    ).unwrap();

    pub static ref POVW_RESOLVE_OPERATIONS: IntCounterVec = IntCounterVec::new(
        Opts::new("povw_resolve_operations_total", "Total number of POVW resolve operations by type"),
        &["operation_type", "status"]
    ).unwrap();

    // General task metrics
    pub static ref TASK_DURATION: Histogram = Histogram::with_opts(
        HistogramOpts::new("task_duration_seconds", "Duration of task execution")
            .buckets(vec![0.1, 0.5, 1.0, 2.5, 5.0, 10.0, 25.0, 50.0, 100.0, 250.0, 500.0])
    ).unwrap();

    pub static ref TASK_OPERATIONS: IntCounterVec = IntCounterVec::new(
        Opts::new("task_operations_total", "Total number of task operations by type and status"),
        &["task_name", "operation_type", "status"]
    ).unwrap();

    // Task-specific timing metrics
    pub static ref PROVE_DURATION: Histogram = Histogram::with_opts(
        HistogramOpts::new("prove_duration_seconds", "Duration of prove operations")
            .buckets(vec![0.1, 0.5, 1.0, 2.5, 5.0, 10.0, 25.0, 50.0, 100.0, 250.0, 500.0])
    ).unwrap();

    pub static ref JOIN_DURATION: Histogram = Histogram::with_opts(
        HistogramOpts::new("join_duration_seconds", "Duration of join operations")
            .buckets(vec![0.1, 0.5, 1.0, 2.5, 5.0, 10.0, 25.0, 50.0, 100.0, 250.0, 500.0])
    ).unwrap();

    pub static ref KECCAK_DURATION: Histogram = Histogram::with_opts(
        HistogramOpts::new("keccak_duration_seconds", "Duration of keccak operations")
            .buckets(vec![0.1, 0.5, 1.0, 2.5, 5.0, 10.0, 25.0, 50.0, 100.0, 250.0, 500.0])
    ).unwrap();

    pub static ref UNION_DURATION: Histogram = Histogram::with_opts(
        HistogramOpts::new("union_duration_seconds", "Duration of union operations")
            .buckets(vec![0.1, 0.5, 1.0, 2.5, 5.0, 10.0, 25.0, 50.0, 100.0, 250.0, 500.0])
    ).unwrap();

    pub static ref RESOLVE_DURATION: Histogram = Histogram::with_opts(
        HistogramOpts::new("resolve_duration_seconds", "Duration of resolve operations")
            .buckets(vec![0.1, 0.5, 1.0, 2.5, 5.0, 10.0, 25.0, 50.0, 100.0, 250.0, 500.0])
    ).unwrap();
}

/// Helper functions for common metric operations
pub mod helpers {
    use super::*;
    use std::time::Instant;

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

    /// Record S3 operation metrics
    pub fn record_s3_operation(operation_type: &str, status: &str, duration_seconds: f64) {
        S3_OPERATIONS.with_label_values(&[operation_type, status]).inc();
        S3_OPERATION_DURATION.observe(duration_seconds);
    }

    /// Record Redis operation metrics
    pub fn record_redis_operation(operation_type: &str, status: &str, duration_seconds: f64) {
        REDIS_OPERATIONS.with_label_values(&[operation_type, status]).inc();
        REDIS_OPERATION_DURATION.observe(duration_seconds);
    }

    /// Record task operation metrics
    pub fn record_task_operation(task_name: &str, operation_type: &str, status: &str) {
        TASK_OPERATIONS.with_label_values(&[task_name, operation_type, status]).inc();
    }

    /// Record POVW resolve operation metrics
    pub fn record_povw_operation(operation_type: &str, status: &str, duration_seconds: f64) {
        POVW_RESOLVE_OPERATIONS.with_label_values(&[operation_type, status]).inc();
        POVW_RESOLVE_DURATION.observe(duration_seconds);
    }

    /// Record database operation metrics
    pub fn record_db_operation(operation_type: &str, status: &str, duration_seconds: f64) {
        DB_OPERATIONS.with_label_values(&[operation_type, status]).inc();
        DB_OPERATION_DURATION.observe(duration_seconds);
    }

    /// Update database connection pool metrics
    pub fn update_db_pool_metrics(size: i64, idle: i64, active: i64) {
        DB_CONNECTION_POOL_SIZE.set(size);
        DB_CONNECTION_POOL_IDLE.set(idle);
        DB_CONNECTION_POOL_ACTIVE.set(active);
    }
}
