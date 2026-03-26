// Copyright 2026 Boundless Foundation, Inc.
//
// Use of this source code is governed by the Business Source License
// as found in the LICENSE-BSL file.

use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::Value as JsonValue;
use thiserror::Error;
use uuid::Uuid;

pub mod planner;
pub mod redis_backend;

#[derive(Error, Debug)]
pub enum TaskDbErr {
    #[error("redis failure")]
    RedisError(#[from] redis::RedisError),
    #[error("taskdb row not found: {0}")]
    NotFound(String),
    #[error("taskdb internal error: {0}")]
    InternalErr(String),
    #[error("json serialization error")]
    JsonErr(#[from] serde_json::Error),
    #[error("stream be_mult cannot be 0.0")]
    InvalidBeMult,
}

impl TaskDbErr {
    pub fn is_not_found(&self) -> bool {
        matches!(self, TaskDbErr::NotFound(_))
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum JobState {
    Running,
    Done,
    Failed,
}

impl std::fmt::Display for JobState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            JobState::Running => write!(f, "RUNNING"),
            JobState::Done => write!(f, "SUCCEEDED"),
            JobState::Failed => write!(f, "FAILED"),
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord, Default)]
#[repr(i32)]
#[serde(rename_all = "lowercase")]
pub enum Priority {
    High = 0,
    #[default]
    Medium = 1,
    Low = 2,
}

impl Priority {
    pub const fn bucket(self) -> i32 {
        self as i32
    }
}

pub struct ReadyTask {
    pub job_id: Uuid,
    pub task_id: String,
    pub task_def: JsonValue,
    pub prereqs: JsonValue,
    pub max_retries: i32,
}

/// Initial taskid in a job.
pub const INIT_TASK: &str = "init";

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TaskState {
    Pending,
    Ready,
    Running,
    Done,
    Failed,
}

impl std::fmt::Display for TaskState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TaskState::Pending => write!(f, "Pending"),
            TaskState::Ready => write!(f, "Ready"),
            TaskState::Running => write!(f, "Running"),
            TaskState::Done => write!(f, "Done"),
            TaskState::Failed => write!(f, "Failed"),
        }
    }
}

#[derive(Clone, Debug)]
pub struct StuckTaskInfo {
    pub job_id: Uuid,
    pub task_id: String,
    pub waiting_on: i32,
    pub actual_deps: i32,
    pub completed_deps: i32,
    pub should_be_ready: bool,
}

#[derive(Clone)]
pub struct TaskDb {
    redis: redis_backend::RedisTaskDb,
}

impl TaskDb {
    pub fn redis(redis: redis_backend::RedisTaskDb) -> Self {
        Self { redis }
    }

    pub fn connect_redis(redis_url: &str, namespace: &str) -> Result<Self, TaskDbErr> {
        Ok(Self { redis: redis_backend::RedisTaskDb::with_namespace(redis_url, namespace)? })
    }

    pub async fn create_stream(
        &self,
        worker_type: &str,
        reserved: i32,
        be_mult: f32,
        user_id: &str,
    ) -> Result<Uuid, TaskDbErr> {
        self.redis.create_stream(worker_type, reserved, be_mult, user_id).await
    }

    pub async fn create_job(
        &self,
        stream_id: &Uuid,
        task_def: &JsonValue,
        max_retries: i32,
        timeout_secs: i32,
        user_id: &str,
    ) -> Result<Uuid, TaskDbErr> {
        self.redis.create_job(stream_id, task_def, max_retries, timeout_secs, user_id).await
    }

    pub async fn create_job_with_priority(
        &self,
        stream_id: &Uuid,
        task_def: &JsonValue,
        max_retries: i32,
        timeout_secs: i32,
        user_id: &str,
        priority: Priority,
    ) -> Result<Uuid, TaskDbErr> {
        self.redis
            .create_job_with_priority(
                stream_id,
                task_def,
                max_retries,
                timeout_secs,
                user_id,
                priority,
            )
            .await
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn create_task(
        &self,
        job_id: &Uuid,
        task_id: &str,
        stream_id: &Uuid,
        task_def: &JsonValue,
        prereqs: &JsonValue,
        max_retries: i32,
        timeout_secs: i32,
    ) -> Result<(), TaskDbErr> {
        self.redis
            .create_task(job_id, task_id, stream_id, task_def, prereqs, max_retries, timeout_secs)
            .await
    }

    pub async fn request_work(&self, worker_type: &str) -> Result<Option<ReadyTask>, TaskDbErr> {
        self.redis.request_work(worker_type).await
    }

    pub async fn request_work_wait(
        &self,
        worker_type: &str,
        timeout_secs: u64,
    ) -> Result<Option<ReadyTask>, TaskDbErr> {
        self.redis.request_work_blocking(worker_type, timeout_secs).await
    }

    pub async fn update_task_done(
        &self,
        job_id: &Uuid,
        task_id: &str,
        output: JsonValue,
    ) -> Result<bool, TaskDbErr> {
        self.redis.update_task_done(job_id, task_id, output).await
    }

    pub async fn update_task_failed(
        &self,
        job_id: &Uuid,
        task_id: &str,
        error: &str,
    ) -> Result<bool, TaskDbErr> {
        self.redis.update_task_failed(job_id, task_id, error).await
    }

    pub async fn update_task_retry(&self, job_id: &Uuid, task_id: &str) -> Result<bool, TaskDbErr> {
        self.redis.update_task_retry(job_id, task_id).await
    }

    pub async fn update_task_progress(
        &self,
        job_id: &Uuid,
        task_id: &str,
        progress: f32,
    ) -> Result<bool, TaskDbErr> {
        self.redis.update_task_progress(job_id, task_id, progress).await
    }

    pub async fn requeue_tasks(&self, limit: i64) -> Result<usize, TaskDbErr> {
        self.redis.requeue_tasks(limit).await
    }

    pub async fn check_stuck_pending_tasks(&self) -> Result<Vec<StuckTaskInfo>, TaskDbErr> {
        Ok(vec![])
    }

    pub async fn fix_stuck_pending_tasks(&self) -> Result<i32, TaskDbErr> {
        Ok(0)
    }

    pub async fn clear_completed_jobs(&self) -> Result<i32, TaskDbErr> {
        self.redis.clear_completed_jobs().await
    }

    pub async fn get_job_state(&self, job_id: &Uuid, user_id: &str) -> Result<JobState, TaskDbErr> {
        self.redis.get_job_state(job_id, user_id).await
    }

    pub async fn get_job_unresolved(&self, job_id: &Uuid) -> Result<i64, TaskDbErr> {
        self.redis.get_job_unresolved(job_id).await
    }

    pub async fn get_concurrent_jobs(&self, user_id: &str) -> Result<i64, TaskDbErr> {
        self.redis.get_concurrent_jobs(user_id).await
    }

    pub async fn get_stream(
        &self,
        user_id: &str,
        worker_type: &str,
    ) -> Result<Option<Uuid>, TaskDbErr> {
        self.redis.get_stream(user_id, worker_type).await
    }

    pub async fn get_job_failure(&self, job_id: &Uuid) -> Result<String, TaskDbErr> {
        self.redis.get_job_failure(job_id).await
    }

    pub async fn get_task_output<T>(&self, job_id: &Uuid, task_id: &str) -> Result<T, TaskDbErr>
    where
        T: DeserializeOwned,
    {
        self.redis.get_task_output(job_id, task_id).await
    }

    pub async fn get_task_retries_running(
        &self,
        job_id: &Uuid,
        task_id: &str,
    ) -> Result<Option<i32>, TaskDbErr> {
        self.redis.get_task_retries_running(job_id, task_id).await
    }

    pub async fn delete_job(&self, job_id: &Uuid) -> Result<(), TaskDbErr> {
        self.redis.delete_job(job_id).await
    }
}
