// Copyright 2025 RISC Zero, Inc.
//
// Use of this source code is governed by the Business Source License
// as found in the LICENSE-BSL file.

use redis::{Client, Commands, Connection};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::Value as JsonValue;
use thiserror::Error;
use uuid::Uuid;

pub mod planner;

#[derive(Error, Debug)]
pub enum TaskDbErr {
    #[error("redis failure")]
    RedisError(#[from] redis::RedisError),
    #[error("taskdb internal error: {0}")]
    InternalErr(String),
    #[error("jsonb serialization error, invalid json in DB")]
    JsonErr(#[from] serde_json::Error),
    #[error("Stream be_mult cannot be 0.0")]
    InvalidBeMult,
    #[error("Task not found")]
    TaskNotFound,
    #[error("Stream not found")]
    StreamNotFound,
    #[error("UUID error: {0}")]
    UuidError(uuid::Error),
}

impl From<uuid::Error> for TaskDbErr {
    fn from(err: uuid::Error) -> Self {
        TaskDbErr::UuidError(err)
    }
}

#[derive(Deserialize, Serialize, Debug, PartialEq, Clone)]
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

#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct ReadyTask {
    pub job_id: Uuid,
    pub task_id: String,
    pub task_def: JsonValue,
    pub prereqs: JsonValue,
    pub max_retries: i32,
}

/// Initial taskid in a job, defined in the SQL schema from create_job()
pub const INIT_TASK: &str = "init";

#[derive(Debug, PartialEq, Clone, Serialize, Deserialize)]
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

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Stream {
    pub id: Uuid,
    pub worker_type: String,
    pub reserved: i32,
    pub be_mult: f32,
    pub user_id: String,
    pub running: i32,
    pub ready: i32,
    pub priority: f32,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Task {
    pub job_id: Uuid,
    pub task_id: String,
    pub stream_id: Uuid,
    pub task_def: JsonValue,
    pub prerequisites: JsonValue,
    pub state: TaskState,
    pub created_at: i64,
    pub started_at: Option<i64>,
    pub updated_at: Option<i64>,
    pub waiting_on: i32,
    pub progress: f32,
    pub retries: i32,
    pub max_retries: i32,
    pub timeout_secs: i32,
    pub output: Option<JsonValue>,
    pub error: Option<String>,
    pub worker_id: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Job {
    pub id: Uuid,
    pub state: JobState,
    pub error: Option<String>,
    pub user_id: String,
    pub reported: bool,
}

pub struct RedisTaskDB {
    client: Client,
    connection: Connection,
}

impl RedisTaskDB {
    pub async fn new(redis_url: &str) -> Result<Self, TaskDbErr> {
        let client = Client::open(redis_url)?;
        let connection = client.get_connection()?;

        Ok(Self { client, connection })
    }

    pub fn get_connection(&self) -> Result<Connection, TaskDbErr> {
        Ok(self.client.get_connection()?)
    }

    // Stream management
    pub async fn create_stream(
        &mut self,
        worker_type: &str,
        reserved: i32,
        be_mult: f32,
        user_id: &str,
    ) -> Result<Uuid, TaskDbErr> {
        if be_mult == 0.0 {
            return Err(TaskDbErr::InvalidBeMult);
        }

        let stream_id = Uuid::new_v4();
        let priority = if reserved == 0 { f32::INFINITY } else { 0.0 };

        let stream = Stream {
            id: stream_id,
            worker_type: worker_type.to_string(),
            reserved,
            be_mult,
            user_id: user_id.to_string(),
            running: 0,
            ready: 0,
            priority,
        };

        let stream_data = serde_json::to_string(&stream)?;
        let stream_key = format!("stream:{}", stream_id);

        // Store stream data
        let _: () = self.connection.hset(&stream_key, "data", stream_data)?;

        // Add to worker type index
        let worker_streams_key = format!("worker_streams:{}", worker_type);
        let _: () = self.connection.sadd(worker_streams_key, stream_id.to_string())?;

        // Add to priority queue
        let priority_key = format!("streams_by_priority:{}", worker_type);
        let _: () = self.connection.zadd(priority_key, stream_id.to_string(), priority)?;

        // Initialize stream counters
        let counters_key = format!("stream:{}:counters", stream_id);
        let _: () = self.connection.hset(&counters_key, "running", 0)?;
        let _: () = self.connection.hset(&counters_key, "ready", 0)?;
        let _: () = self.connection.hset(&counters_key, "pending", 0)?;

        Ok(stream_id)
    }

    pub async fn create_job(
        &mut self,
        stream_id: &Uuid,
        task_def: &JsonValue,
        max_retries: i32,
        timeout_secs: i32,
        user_id: &str,
    ) -> Result<Uuid, TaskDbErr> {
        let job_id = Uuid::new_v4();

        let job = Job {
            id: job_id,
            state: JobState::Running,
            error: None,
            user_id: user_id.to_string(),
            reported: false,
        };

        let job_data = serde_json::to_string(&job)?;
        let job_key = format!("job:{}", job_id);

        // Store job data
        let _: () = self.connection.hset(&job_key, "data", job_data)?;

        // Create init task
        let init_task = Task {
            job_id,
            task_id: INIT_TASK.to_string(),
            stream_id: *stream_id,
            task_def: task_def.clone(),
            prerequisites: JsonValue::Array(vec![]),
            state: TaskState::Ready,
            created_at: chrono::Utc::now().timestamp(),
            started_at: None,
            updated_at: None,
            waiting_on: 0,
            progress: 0.0,
            retries: 0,
            max_retries,
            timeout_secs,
            output: None,
            error: None,
            worker_id: None,
        };

        self.store_task(&init_task)?;
        self.add_task_to_ready_queue(&init_task)?;
        self.update_stream_counters(stream_id, "ready", 1)?;

        Ok(job_id)
    }

    pub async fn create_task(
        &mut self,
        job_id: &Uuid,
        task_id: &str,
        stream_id: &Uuid,
        task_def: &JsonValue,
        prereqs: &JsonValue,
        max_retries: i32,
        timeout_secs: i32,
    ) -> Result<(), TaskDbErr> {
        let prereqs_list: Vec<String> = serde_json::from_value(prereqs.clone())?;
        let waiting_on = prereqs_list.len() as i32;

        let task = Task {
            job_id: *job_id,
            task_id: task_id.to_string(),
            stream_id: *stream_id,
            task_def: task_def.clone(),
            prerequisites: prereqs.clone(),
            state: if waiting_on == 0 { TaskState::Ready } else { TaskState::Pending },
            created_at: chrono::Utc::now().timestamp(),
            started_at: None,
            updated_at: None,
            waiting_on,
            progress: 0.0,
            retries: 0,
            max_retries,
            timeout_secs,
            output: None,
            error: None,
            worker_id: None,
        };

        self.store_task(&task)?;

        // Store dependencies
        for prereq in prereqs_list {
            let dep_key = format!("deps:{}:{}", job_id, task_id);
            let _: () = self.connection.sadd(dep_key, prereq)?;
        }

        if waiting_on == 0 {
            self.add_task_to_ready_queue(&task)?;
            self.update_stream_counters(stream_id, "ready", 1)?;
        } else {
            self.add_task_to_pending_queue(&task)?;
            self.update_stream_counters(stream_id, "pending", 1)?;
        }

        Ok(())
    }

    pub async fn request_work(
        &mut self,
        worker_type: &str,
    ) -> Result<Option<ReadyTask>, TaskDbErr> {
        // Find highest priority stream with work
        let priority_key = format!("streams_by_priority:{}", worker_type);
        let streams: Vec<String> = self.connection.zrange(priority_key, 0, 0)?;

        if streams.is_empty() {
            return Ok(None);
        }

        let stream_id = Uuid::parse_str(&streams[0])?;
        let ready_key = format!("stream:{}:ready", stream_id);

        // Atomically pop task from ready queue
        let task_key: Option<String> = self.connection.rpop(ready_key, None)?;
        if task_key.is_none() {
            return Ok(None);
        }

        let task_key = task_key.unwrap();

        // Get task data
        let task_data: String = self.connection.hget(&task_key, "data")?;
        let mut task: Task = serde_json::from_str(&task_data)?;

        // Update task state
        task.state = TaskState::Running;
        task.started_at = Some(chrono::Utc::now().timestamp());

        let updated_data = serde_json::to_string(&task)?;
        let _: () = self.connection.hset(&task_key, "data", updated_data)?;

        // Update stream counters
        self.update_stream_counters(&stream_id, "running", 1)?;
        self.update_stream_counters(&stream_id, "ready", -1)?;

        // Update stream priority
        self.update_stream_priority(&stream_id)?;

        Ok(Some(ReadyTask {
            job_id: task.job_id,
            task_id: task.task_id,
            task_def: task.task_def,
            prereqs: task.prerequisites,
            max_retries: task.max_retries,
        }))
    }

    pub async fn update_task_done(
        &mut self,
        job_id: &Uuid,
        task_id: &str,
        output: JsonValue,
    ) -> Result<bool, TaskDbErr> {
        let task_key = format!("task:{}:{}", job_id, task_id);

        // Get current task data
        let task_data: String = match self.connection.hget(&task_key, "data") {
            Ok(data) => data,
            Err(_) => return Ok(false),
        };

        let mut task: Task = serde_json::from_str(&task_data)?;

        // Check if task can be updated
        if task.state != TaskState::Ready && task.state != TaskState::Running {
            return Ok(false);
        }

        // Update task
        task.state = TaskState::Done;
        task.output = Some(output);
        task.updated_at = Some(chrono::Utc::now().timestamp());
        task.progress = 1.0;

        let updated_data = serde_json::to_string(&task)?;
        let _: () = self.connection.hset(&task_key, "data", updated_data)?;

        // Update stream counters
        self.update_stream_counters(&task.stream_id, "running", -1)?;

        // Check dependent tasks
        self.check_dependent_tasks(job_id, task_id)?;

        // Check if job is complete
        self.check_job_completion(job_id).await?;

        Ok(true)
    }

    pub async fn update_task_failed(
        &mut self,
        job_id: &Uuid,
        task_id: &str,
        error: &str,
    ) -> Result<bool, TaskDbErr> {
        let task_key = format!("task:{}:{}", job_id, task_id);

        // Get current task data
        let task_data: String = match self.connection.hget(&task_key, "data") {
            Ok(data) => data,
            Err(_) => return Ok(false),
        };

        let mut task: Task = serde_json::from_str(&task_data)?;

        // Check if task can be updated
        if task.state != TaskState::Ready
            && task.state != TaskState::Running
            && task.state != TaskState::Pending
        {
            return Ok(false);
        }

        // Update task
        task.state = TaskState::Failed;
        task.error = Some(error.to_string());
        task.updated_at = Some(chrono::Utc::now().timestamp());
        task.progress = 1.0;

        let updated_data = serde_json::to_string(&task)?;
        let _: () = self.connection.hset(&task_key, "data", updated_data)?;

        // Update stream counters
        if task.state == TaskState::Running {
            self.update_stream_counters(&task.stream_id, "running", -1)?;
        } else if task.state == TaskState::Ready {
            self.update_stream_counters(&task.stream_id, "ready", -1)?;
        } else if task.state == TaskState::Pending {
            self.update_stream_counters(&task.stream_id, "pending", -1)?;
        }

        // Update job state
        self.update_job_state(job_id, JobState::Failed, Some(error.to_string()))?;

        Ok(true)
    }

    pub async fn update_task_progress(
        &mut self,
        job_id: &Uuid,
        task_id: &str,
        progress: f32,
    ) -> Result<bool, TaskDbErr> {
        let task_key = format!("task:{}:{}", job_id, task_id);

        // Get current task data
        let task_data: String = match self.connection.hget(&task_key, "data") {
            Ok(data) => data,
            Err(_) => return Ok(false),
        };

        let mut task: Task = serde_json::from_str(&task_data)?;

        // Check if task can be updated
        if task.state != TaskState::Ready && task.state != TaskState::Running {
            return Ok(false);
        }

        // Update progress (only increase)
        if progress > task.progress {
            task.progress = progress;
            task.updated_at = Some(chrono::Utc::now().timestamp());

            let updated_data = serde_json::to_string(&task)?;
            let _: () = self.connection.hset(&task_key, "data", updated_data)?;
        }

        Ok(true)
    }

    pub async fn update_task_retry(
        &mut self,
        job_id: &Uuid,
        task_id: &str,
    ) -> Result<bool, TaskDbErr> {
        let task_key = format!("task:{}:{}", job_id, task_id);

        // Get current task data
        let task_data: String = match self.connection.hget(&task_key, "data") {
            Ok(data) => data,
            Err(_) => return Ok(false),
        };

        let mut task: Task = serde_json::from_str(&task_data)?;

        // Check if task can be retried
        if task.state != TaskState::Running {
            return Ok(false);
        }

        task.retries += 1;

        // Check if max retries exceeded
        if task.retries > task.max_retries {
            return self.update_task_failed(job_id, task_id, "retry max hit").await;
        }

        // Reset task for retry
        task.state = TaskState::Ready;
        task.progress = 0.0;
        task.error = None;
        task.updated_at = Some(chrono::Utc::now().timestamp());
        task.started_at = None;
        task.worker_id = None;

        let updated_data = serde_json::to_string(&task)?;
        let _: () = self.connection.hset(&task_key, "data", updated_data)?;

        // Update stream counters
        self.update_stream_counters(&task.stream_id, "running", -1)?;
        self.update_stream_counters(&task.stream_id, "ready", 1)?;

        // Add back to ready queue
        self.add_task_to_ready_queue(&task)?;

        Ok(true)
    }

    pub async fn requeue_tasks(&mut self, limit: i64) -> Result<usize, TaskDbErr> {
        let now = chrono::Utc::now().timestamp();
        let mut requeued = 0;

        // Find timed out tasks
        let pattern = "task:*";
        let keys: Vec<String> = self.connection.keys(pattern)?;

        for task_key in keys {
            if requeued >= limit as usize {
                break;
            }

            let task_data: String = match self.connection.hget(&task_key, "data") {
                Ok(data) => data,
                Err(_) => continue,
            };

            let task: Task = serde_json::from_str(&task_data)?;

            if task.state == TaskState::Running {
                let timeout_time =
                    task.started_at.unwrap_or(task.created_at) + task.timeout_secs as i64;
                if now > timeout_time {
                    self.update_task_retry(&task.job_id, &task.task_id).await?;
                    requeued += 1;
                }
            }
        }

        Ok(requeued)
    }

    pub async fn get_job_state(
        &mut self,
        job_id: &Uuid,
        user_id: &str,
    ) -> Result<JobState, TaskDbErr> {
        let job_key = format!("job:{}", job_id);
        let job_data: String = self.connection.hget(&job_key, "data")?;
        let job: Job = serde_json::from_str(&job_data)?;

        if job.user_id != user_id {
            return Err(TaskDbErr::InternalErr("Job not found or access denied".to_string()));
        }

        Ok(job.state)
    }

    pub async fn get_job_unresolved(&mut self, job_id: &Uuid) -> Result<i64, TaskDbErr> {
        let pattern = format!("task:{}:*", job_id);
        let keys: Vec<String> = self.connection.keys(pattern)?;

        let mut unresolved = 0;
        for task_key in keys {
            let task_data: String = self.connection.hget(&task_key, "data")?;
            let task: Task = serde_json::from_str(&task_data)?;

            if task.state != TaskState::Done {
                unresolved += 1;
            }
        }

        Ok(unresolved)
    }

    pub async fn get_concurrent_jobs(&mut self, user_id: &str) -> Result<i64, TaskDbErr> {
        let pattern = "job:*";
        let keys: Vec<String> = self.connection.keys(pattern)?;

        let mut count = 0;
        for job_key in keys {
            let job_data: String = self.connection.hget(&job_key, "data")?;
            let job: Job = serde_json::from_str(&job_data)?;

            if job.user_id == user_id && job.state == JobState::Running {
                count += 1;
            }
        }

        Ok(count)
    }

    pub async fn get_stream(
        &mut self,
        user_id: &str,
        worker_type: &str,
    ) -> Result<Option<Uuid>, TaskDbErr> {
        let worker_streams_key = format!("worker_streams:{}", worker_type);
        let streams: Vec<String> = self.connection.smembers(worker_streams_key)?;

        for stream_id_str in streams {
            let stream_id = Uuid::parse_str(&stream_id_str)?;
            let stream_key = format!("stream:{}", stream_id);
            let stream_data: String = self.connection.hget(&stream_key, "data")?;
            let stream: Stream = serde_json::from_str(&stream_data)?;

            if stream.user_id == user_id {
                return Ok(Some(stream_id));
            }
        }

        Ok(None)
    }

    pub async fn get_job_time(&mut self, job_id: &Uuid) -> Result<f64, TaskDbErr> {
        let pattern = format!("task:{}:*", job_id);
        let keys: Vec<String> = self.connection.keys(pattern)?;

        let mut min_started = i64::MAX;
        let mut max_updated = 0i64;

        for task_key in keys {
            let task_data: String = self.connection.hget(&task_key, "data")?;
            let task: Task = serde_json::from_str(&task_data)?;

            if let Some(started) = task.started_at {
                if started < min_started {
                    min_started = started;
                }
            }

            if let Some(updated) = task.updated_at {
                if updated > max_updated {
                    max_updated = updated;
                }
            }
        }

        if min_started == i64::MAX {
            return Ok(0.0);
        }

        Ok((max_updated - min_started) as f64)
    }

    pub async fn get_job_failure(&mut self, job_id: &Uuid) -> Result<String, TaskDbErr> {
        let pattern = format!("task:{}:*", job_id);
        let keys: Vec<String> = self.connection.keys(pattern)?;

        for task_key in keys {
            let task_data: String = self.connection.hget(&task_key, "data")?;
            let task: Task = serde_json::from_str(&task_data)?;

            if task.state == TaskState::Failed {
                if let Some(error) = task.error {
                    if !error.is_empty() {
                        return Ok(error);
                    }
                }
            }
        }

        Err(TaskDbErr::InternalErr("No failed task found".to_string()))
    }

    pub async fn get_task_output<T>(&mut self, job_id: &Uuid, task_id: &str) -> Result<T, TaskDbErr>
    where
        T: DeserializeOwned,
    {
        let task_key = format!("task:{}:{}", job_id, task_id);
        let task_data: String = self.connection.hget(&task_key, "data")?;
        let task: Task = serde_json::from_str(&task_data)?;

        if task.state != TaskState::Done {
            return Err(TaskDbErr::InternalErr("Task not completed".to_string()));
        }

        if let Some(output) = task.output {
            Ok(serde_json::from_value(output)?)
        } else {
            Err(TaskDbErr::InternalErr("No output available".to_string()))
        }
    }

    pub async fn delete_job(&mut self, job_id: &Uuid) -> Result<(), TaskDbErr> {
        // Delete job
        let job_key = format!("job:{}", job_id);
        let _: () = self.connection.del(&job_key)?;

        // Delete all tasks for this job
        let pattern = format!("task:{}:*", job_id);
        let keys: Vec<String> = self.connection.keys(pattern)?;

        if !keys.is_empty() {
            let _: () = self.connection.del(keys)?;
        }

        // Delete dependencies
        let dep_pattern = format!("deps:{}:*", job_id);
        let dep_keys: Vec<String> = self.connection.keys(dep_pattern)?;

        if !dep_keys.is_empty() {
            let _: () = self.connection.del(dep_keys)?;
        }

        Ok(())
    }

    // Helper methods
    fn store_task(&mut self, task: &Task) -> Result<(), TaskDbErr> {
        let task_key = format!("task:{}:{}", task.job_id, task.task_id);
        let task_data = serde_json::to_string(task)?;
        let _: () = self.connection.hset(&task_key, "data", task_data)?;
        Ok(())
    }

    fn add_task_to_ready_queue(&mut self, task: &Task) -> Result<(), TaskDbErr> {
        let ready_key = format!("stream:{}:ready", task.stream_id);
        let task_key = format!("task:{}:{}", task.job_id, task.task_id);
        let _: () = self.connection.lpush(ready_key, task_key)?;
        Ok(())
    }

    fn add_task_to_pending_queue(&mut self, task: &Task) -> Result<(), TaskDbErr> {
        let pending_key = format!("stream:{}:pending", task.stream_id);
        let task_key = format!("task:{}:{}", task.job_id, task.task_id);
        let _: () = self.connection.lpush(pending_key, task_key)?;
        Ok(())
    }

    fn update_stream_counters(
        &mut self,
        stream_id: &Uuid,
        counter: &str,
        delta: i32,
    ) -> Result<(), TaskDbErr> {
        let counters_key = format!("stream:{}:counters", stream_id);
        let _: () = self.connection.hincr(&counters_key, counter, delta)?;

        // Update stream data
        let stream_key = format!("stream:{}", stream_id);
        let stream_data: String = self.connection.hget(&stream_key, "data")?;
        let mut stream: Stream = serde_json::from_str(&stream_data)?;

        match counter {
            "running" => stream.running = (stream.running + delta).max(0),
            "ready" => stream.ready = (stream.ready + delta).max(0),
            "pending" => {} // We don't track pending in the stream object
            _ => {}
        }

        // Recalculate priority
        stream.priority = if stream.ready == 0 {
            f32::INFINITY
        } else if stream.running >= stream.reserved {
            (stream.running - stream.reserved) as f32 / stream.be_mult
        } else {
            (stream.running - stream.reserved) as f32 / stream.reserved as f32
        };

        let updated_data = serde_json::to_string(&stream)?;
        let _: () = self.connection.hset(&stream_key, "data", updated_data)?;

        // Update priority queue
        let priority_key = format!("streams_by_priority:{}", stream.worker_type);
        let _: () = self.connection.zadd(priority_key, stream_id.to_string(), stream.priority)?;

        Ok(())
    }

    fn update_stream_priority(&mut self, stream_id: &Uuid) -> Result<(), TaskDbErr> {
        let stream_key = format!("stream:{}", stream_id);
        let stream_data: String = self.connection.hget(&stream_key, "data")?;
        let mut stream: Stream = serde_json::from_str(&stream_data)?;

        // Get current counters
        let counters_key = format!("stream:{}:counters", stream_id);
        let running: i32 = self.connection.hget(&counters_key, "running")?;
        let ready: i32 = self.connection.hget(&counters_key, "ready")?;

        stream.running = running;
        stream.ready = ready;

        // Recalculate priority
        stream.priority = if stream.ready == 0 {
            f32::INFINITY
        } else if stream.running >= stream.reserved {
            (stream.running - stream.reserved) as f32 / stream.be_mult
        } else {
            (stream.running - stream.reserved) as f32 / stream.reserved as f32
        };

        let updated_data = serde_json::to_string(&stream)?;
        let _: () = self.connection.hset(&stream_key, "data", updated_data)?;

        // Update priority queue
        let priority_key = format!("streams_by_priority:{}", stream.worker_type);
        let _: () = self.connection.zadd(priority_key, stream_id.to_string(), stream.priority)?;

        Ok(())
    }

    fn check_dependent_tasks(
        &mut self,
        job_id: &Uuid,
        completed_task_id: &str,
    ) -> Result<(), TaskDbErr> {
        // Find all tasks that depend on this task
        let pattern = format!("deps:{}:*", job_id);
        let dep_keys: Vec<String> = self.connection.keys(pattern)?;

        for dep_key in dep_keys {
            // Check if this dependency exists
            let exists: bool = self.connection.sismember(&dep_key, completed_task_id)?;
            if exists {
                // Remove the dependency
                let _: () = self.connection.srem(&dep_key, completed_task_id)?;

                // Get the dependent task
                let task_id = dep_key.split(':').nth(2).unwrap();
                let task_key = format!("task:{}:{}", job_id, task_id);
                let task_data: String = self.connection.hget(&task_key, "data")?;
                let mut task: Task = serde_json::from_str(&task_data)?;

                // Decrement waiting_on
                task.waiting_on -= 1;

                // If no more dependencies, move to ready
                if task.waiting_on == 0 && task.state == TaskState::Pending {
                    task.state = TaskState::Ready;

                    let updated_data = serde_json::to_string(&task)?;
                    let _: () = self.connection.hset(&task_key, "data", updated_data)?;

                    // Move from pending to ready queue
                    let pending_key = format!("stream:{}:pending", task.stream_id);
                    let ready_key = format!("stream:{}:ready", task.stream_id);

                    // Remove from pending
                    let _: () = self.connection.lrem(&pending_key, 1, &task_key)?;
                    // Add to ready
                    let _: () = self.connection.lpush(ready_key, &task_key)?;

                    // Update counters
                    self.update_stream_counters(&task.stream_id, "pending", -1)?;
                    self.update_stream_counters(&task.stream_id, "ready", 1)?;
                } else {
                    // Just update the waiting_on count
                    let updated_data = serde_json::to_string(&task)?;
                    let _: () = self.connection.hset(&task_key, "data", updated_data)?;
                }
            }
        }

        Ok(())
    }

    async fn check_job_completion(&mut self, job_id: &Uuid) -> Result<(), TaskDbErr> {
        let unresolved = self.get_job_unresolved(job_id).await?;

        if unresolved == 0 {
            // All tasks are done, update job state
            self.update_job_state(job_id, JobState::Done, None)?;
        }

        Ok(())
    }

    fn update_job_state(
        &mut self,
        job_id: &Uuid,
        state: JobState,
        error: Option<String>,
    ) -> Result<(), TaskDbErr> {
        let job_key = format!("job:{}", job_id);
        let job_data: String = self.connection.hget(&job_key, "data")?;
        let mut job: Job = serde_json::from_str(&job_data)?;

        job.state = state;
        job.error = error;

        let updated_data = serde_json::to_string(&job)?;
        let _: () = self.connection.hset(&job_key, "data", updated_data)?;

        Ok(())
    }
}

// Test helpers (similar to original)
pub mod test_helpers {
    use super::*;

    pub async fn cleanup(db: &mut RedisTaskDB) -> Result<(), TaskDbErr> {
        // Clear all keys
        let pattern = "*";
        let keys: Vec<String> = db.connection.keys(pattern)?;
        if !keys.is_empty() {
            let _: () = db.connection.del(keys)?;
        }
        Ok(())
    }

    pub async fn get_task(
        db: &mut RedisTaskDB,
        job_id: &Uuid,
        task_id: &str,
    ) -> Result<Task, TaskDbErr> {
        let task_key = format!("task:{}:{}", job_id, task_id);
        let task_data: String = db.connection.hget(&task_key, "data")?;
        Ok(serde_json::from_str(&task_data)?)
    }

    pub async fn get_job(db: &mut RedisTaskDB, job_id: &Uuid) -> Result<Job, TaskDbErr> {
        let job_key = format!("job:{}", job_id);
        let job_data: String = db.connection.hget(&job_key, "data")?;
        Ok(serde_json::from_str(&job_data)?)
    }
}
