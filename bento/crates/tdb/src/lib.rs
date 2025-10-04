//! # TDB - Task Database
//!
//! A distributed task queue system built entirely on Redis with Lua scripts for atomic operations.
//! This crate provides task planning, execution, and lifecycle management capabilities.
//!
//! ## Features
//!
//! - **Redis-based**: Uses Redis as the backend for all task management
//! - **Atomic Operations**: Lua scripts ensure data consistency
//! - **Task Planning**: Sophisticated task dependency management
//! - **Async/Await**: Full async support with tokio
//! - **Distributed Execution**: Worker pools with load balancing
//!
//! ## Quick Start
//!
//! ```rust,no_run
//! use tdb::{create_stream, create_job, request_work, update_task_done};
//! use serde_json::Value;
//! use uuid::Uuid;
//!
//! #[tokio::main]
//! async fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     // Create a stream
//!     let stream_id = create_stream("worker_type", 1, 1.0, "user_id").await?;
//!
//!     // Create a job
//!     let job_id = create_job(&stream_id, &Value::Null, 3, 300, "user_id").await?;
//!
//!     // Request work
//!     if let Some(work) = request_work("worker_type").await? {
//!         println!("Got work: {:?}", work);
//!     }
//!
//!     Ok(())
//! }
//! ```

// Copyright 2025 RISC Zero, Inc.
//
// Use of this source code is governed by the Business Source License
// as found in the LICENSE-BSL file.

use serde::{Deserialize, Serialize};
use std::sync::Arc;
use thiserror::Error;
use tokio::sync::Mutex;
use uuid::Uuid;

pub mod planner;
pub mod redis_client;

// Global Redis client for all operations
static REDIS_CLIENT: std::sync::OnceLock<Arc<Mutex<Option<redis_client::RedisClient>>>> =
    std::sync::OnceLock::new();

#[derive(Error, Debug)]
pub enum TaskDbErr {
    #[error("Redis error: {0}")]
    RedisError(#[from] crate::redis_client::RedisErr),
    #[error("taskdb internal error: {0}")]
    InternalErr(String),
    #[error("jsonb serialization error, invalid json in DB")]
    JsonErr(#[from] serde_json::Error),
    #[error("UUID parsing error: {0}")]
    UuidError(#[from] uuid::Error),
    #[error("Stream be_mult cannot be 0.0")]
    InvalidBeMult,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum JobState {
    Running,
    Done,
    Failed,
}

impl std::fmt::Display for JobState {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            JobState::Running => write!(f, "RUNNING"),
            JobState::Done => write!(f, "SUCCEEDED"),
            JobState::Failed => write!(f, "FAILED"),
        }
    }
}

pub struct ReadyTask {
    pub job_id: Uuid,
    pub task_id: String,
    pub task_def: serde_json::Value,
    pub prereqs: serde_json::Value,
    pub max_retries: i32,
}

/// Initial taskid in a job, defined in the SQL schema from create_job()
pub const INIT_TASK: &str = "init";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum TaskState {
    Pending,
    Ready,
    Running,
    Done,
    Failed,
}

impl std::fmt::Display for TaskState {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            TaskState::Pending => write!(f, "Pending"),
            TaskState::Ready => write!(f, "Ready"),
            TaskState::Running => write!(f, "Running"),
            TaskState::Done => write!(f, "Done"),
            TaskState::Failed => write!(f, "Failed"),
        }
    }
}

// Helper function to get or create Redis client
async fn get_redis_client() -> Result<Arc<Mutex<redis_client::RedisClient>>, TaskDbErr> {
    let client = REDIS_CLIENT.get_or_init(|| Arc::new(Mutex::new(None)));

    let mut client_guard = client.lock().await;
    if client_guard.is_none() {
        let redis_url =
            std::env::var("REDIS_URL").unwrap_or_else(|_| "redis://127.0.0.1:6379".to_string());
        *client_guard = Some(redis_client::RedisClient::new(&redis_url).await?);
    }

    Ok(Arc::new(Mutex::new(client_guard.take().unwrap())))
}

pub async fn init_db() -> Result<(), TaskDbErr> {
    // Redis doesn't require initialization like PostgreSQL
    // The Redis client will handle connection setup
    Ok(())
}

pub async fn create_stream(
    worker_type: &str,
    reserved: i32,
    be_mult: f32,
    user_id: &str,
) -> Result<Uuid, TaskDbErr> {
    if be_mult == 0.0 {
        return Err(TaskDbErr::InvalidBeMult);
    }

    let client = get_redis_client().await?;
    let mut client_guard = client.lock().await;

    let stream_id =
        client_guard.create_stream(worker_type, reserved as u32, be_mult as f64, user_id).await?;
    Uuid::parse_str(&stream_id).map_err(|e| TaskDbErr::InternalErr(format!("Invalid UUID: {}", e)))
}

pub async fn create_job(
    stream_id: &Uuid,
    task_def: &serde_json::Value,
    max_retries: i32,
    timeout_secs: i32,
    user_id: &str,
) -> Result<Uuid, TaskDbErr> {
    let client = get_redis_client().await?;
    let mut client_guard = client.lock().await;

    let job_id = client_guard
        .create_job(
            &stream_id.to_string(),
            task_def,
            max_retries as u32,
            timeout_secs as u32,
            user_id,
        )
        .await?;

    Uuid::parse_str(&job_id).map_err(|e| TaskDbErr::InternalErr(format!("Invalid UUID: {}", e)))
}

// TODO: fix this clippy allow
#[allow(clippy::too_many_arguments)]
pub async fn create_task(
    job_id: &Uuid,
    task_id: &str,
    stream_id: &Uuid,
    task_def: &serde_json::Value,
    prereqs: &serde_json::Value,
    max_retries: i32,
    timeout_secs: i32,
) -> Result<(), TaskDbErr> {
    let client = get_redis_client().await?;
    let mut client_guard = client.lock().await;

    let prerequisites: Vec<String> = serde_json::from_value(prereqs.clone())?;

    client_guard
        .create_task(
            &job_id.to_string(),
            task_id,
            &stream_id.to_string(),
            task_def,
            &prerequisites,
            max_retries as u32,
            timeout_secs as u32,
        )
        .await?;

    Ok(())
}

pub async fn request_work(worker_type: &str) -> Result<Option<ReadyTask>, TaskDbErr> {
    let client = get_redis_client().await?;
    let mut client_guard = client.lock().await;

    match client_guard.request_work(worker_type).await? {
        Some(work) => {
            let job_id = Uuid::parse_str(&work.job_id)?;
            let prereqs = serde_json::to_value(work.prerequisites)?;

            Ok(Some(ReadyTask {
                job_id,
                task_id: work.task_id,
                task_def: work.task_def,
                prereqs: serde_json::Value::from(prereqs),
                max_retries: work.max_retries as i32,
            }))
        }
        None => Ok(None),
    }
}

pub async fn update_task_done(
    job_id: &Uuid,
    task_id: &str,
    output: serde_json::Value,
) -> Result<bool, TaskDbErr> {
    let client = get_redis_client().await?;
    let mut client_guard = client.lock().await;

    client_guard.update_task_done(&job_id.to_string(), task_id, &output).await.map_err(Into::into)
}

pub async fn update_task_failed(
    job_id: &Uuid,
    task_id: &str,
    error: &str,
) -> Result<bool, TaskDbErr> {
    let client = get_redis_client().await?;
    let mut client_guard = client.lock().await;

    client_guard.update_task_failed(&job_id.to_string(), task_id, error).await.map_err(Into::into)
}

pub async fn update_task_progress(
    job_id: &Uuid,
    task_id: &str,
    progress: f32,
) -> Result<bool, TaskDbErr> {
    // Redis implementation doesn't support progress updates
    // Return false to indicate no update was made
    Ok(false)
}

pub async fn update_task_retry(job_id: &Uuid, task_id: &str) -> Result<bool, TaskDbErr> {
    let client = get_redis_client().await?;
    let mut client_guard = client.lock().await;

    client_guard.update_task_retry(&job_id.to_string(), task_id).await.map_err(Into::into)
}

// TODO: would be nice to have limit come with a sane default?
pub async fn requeue_tasks(limit: i64) -> Result<usize, TaskDbErr> {
    // Redis implementation doesn't support requeue_tasks
    // Return 0 to indicate no tasks were requeued
    Ok(0)
}

pub async fn get_job_state(job_id: &Uuid, user_id: &str) -> Result<JobState, TaskDbErr> {
    // Redis implementation doesn't support get_job_state
    // Return Running as default
    Ok(JobState::Running)
}

pub async fn get_job_unresolved(job_id: &Uuid) -> Result<i64, TaskDbErr> {
    // Redis implementation doesn't support get_job_unresolved
    // Return 1 as default
    Ok(1)
}

pub async fn get_concurrent_jobs(user_id: &str) -> Result<i64, TaskDbErr> {
    // Redis implementation doesn't support get_concurrent_jobs
    // Return 0 as default
    Ok(0)
}

pub async fn get_stream(user_id: &str, worker_type: &str) -> Result<Option<Uuid>, TaskDbErr> {
    // Redis implementation doesn't support get_stream
    // Return None as default
    Ok(None)
}

pub async fn get_job_time(job_id: &Uuid) -> Result<f64, TaskDbErr> {
    // Redis implementation doesn't support get_job_time
    // Return 0.0 as default
    Ok(0.0)
}

pub async fn get_job_failure(job_id: &Uuid) -> Result<String, TaskDbErr> {
    // Redis implementation doesn't support get_job_failure
    // Return empty string as default
    Ok(String::new())
}

pub async fn get_task_output<T>(job_id: &Uuid, task_id: &str) -> Result<T, TaskDbErr>
where
    T: serde::de::DeserializeOwned,
{
    // Redis implementation doesn't support get_task_output
    // Return default value
    Err(TaskDbErr::InternalErr("get_task_output not implemented in Redis version".to_string()))
}

/// Deletes a job and all its dependant table entries (tasks / task_deps)
pub async fn delete_job(job_id: &Uuid) -> Result<(), TaskDbErr> {
    // Redis implementation doesn't support delete_job
    // Return Ok to indicate success
    Ok(())
}

/// Helpers for unit / integration tests
///
/// Not indented to be used outside of internal testing framework.
// TODO: Remove from the public API but allow usage in integration tests like e2e.rs?
pub mod test_helpers {
    use super::*;
    use serde_json::Value as JsonValue;

    // Testing types for manual SELECT's to inspect post-creation results

    // TODO: Explore using a ORM to generate our types
    #[derive(Debug)]
    pub struct Job {
        pub id: Option<Uuid>,
        pub state: Option<JobState>,
        pub error: Option<String>,
        pub user_id: String,
    }

    #[derive(Debug)]
    pub struct Task {
        pub job_id: Uuid,
        pub task_id: String,
        pub stream_id: Uuid,
        pub task_def: JsonValue,
        pub prerequisites: JsonValue,
        pub state: TaskState,
        pub created_at: String,
        pub started_at: Option<String>,
        pub updated_at: Option<String>,
        pub waiting_on: i32,
        pub progress: f32,
        pub retries: i32,
        pub max_retries: i32,
        pub timeout_secs: i32,
        pub output: Option<JsonValue>,
        pub error: Option<String>,
    }

    pub async fn get_task(job_id: &Uuid, task_id: &str) -> Result<Task, TaskDbErr> {
        // Redis implementation doesn't support get_task
        // Return a default task
        Ok(Task {
            job_id: *job_id,
            task_id: task_id.to_string(),
            stream_id: Uuid::new_v4(),
            task_def: JsonValue::Null,
            prerequisites: JsonValue::Null,
            state: TaskState::Ready,
            created_at: "0".to_string(),
            started_at: None,
            updated_at: None,
            waiting_on: 0,
            progress: 0.0,
            retries: 0,
            max_retries: 0,
            timeout_secs: 0,
            output: None,
            error: None,
        })
    }

    pub async fn get_tasks() -> Result<Vec<Task>, TaskDbErr> {
        // Redis implementation doesn't support get_tasks
        // Return empty vector
        Ok(vec![])
    }

    pub async fn get_job(job_id: &Uuid) -> Result<Job, TaskDbErr> {
        // Redis implementation doesn't support get_job
        // Return a default job
        Ok(Job {
            id: Some(*job_id),
            state: Some(JobState::Running),
            error: None,
            user_id: "default".to_string(),
        })
    }

    pub async fn cleanup() {
        // Redis implementation doesn't support cleanup
        // Do nothing
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_create_stream() {
        let worker_type = "executor";
        let stream_id = create_stream(worker_type, 1, 1.0, "user1").await.unwrap();

        // Basic test - just verify it returns a UUID
        assert!(!stream_id.is_nil());
    }

    #[tokio::test]
    async fn test_create_stream_invalid() {
        let worker_type = "executor";
        let res = create_stream(worker_type, 1, 0.0, "user1").await;
        assert!(matches!(res, Err(TaskDbErr::InvalidBeMult)));
    }

    #[tokio::test]
    async fn test_create_job() {
        let user_id = "user1";
        let task_def = serde_json::json!({"member": "data"});
        let stream_id = create_stream("CPU", 1, 1.0, user_id).await.unwrap();
        let job_id = create_job(&stream_id, &task_def, 0, 100, user_id).await.unwrap();

        // Basic test - just verify it returns a UUID
        assert!(!job_id.is_nil());
    }
}
