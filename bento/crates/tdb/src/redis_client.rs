//! # Redis Client
//!
//! The Redis client provides a high-level interface for interacting with Redis
//! using Lua scripts for atomic operations. It handles task creation, lifecycle
//! management, and work distribution.
//!
//! ## Features
//!
//! - **Lua Scripts**: Atomic operations for task management
//! - **Error Handling**: Comprehensive error types and handling
//! - **Async Operations**: Full async/await support
//! - **Script Loading**: Automatic loading of Lua scripts from the filesystem
//!
//! ## Usage
//!
//! ```rust,no_run
//! use tdb::redis_client::RedisClient;
//! use serde_json::Value;
//!
//! #[tokio::main]
//! async fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     let mut client = RedisClient::new("redis://127.0.0.1:6379").await?;
//!
//!     // Create a stream
//!     let stream_id = client.create_stream("worker_type", 1, 1.0, "user_id").await?;
//!
//!     // Create a job
//!     let job_id = client.create_job(&stream_id, &Value::Null, 3, 300, "user_id").await?;
//!
//!     // Request work
//!     if let Some(work) = client.request_work("worker_type").await? {
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

use redis::{Client, RedisResult, Script, aio::Connection};
use serde_json::Value;
use std::collections::HashMap;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum RedisErr {
    #[error("Redis connection error: {0}")]
    ConnectionError(#[from] redis::RedisError),
    #[error("Script execution error: {0}")]
    ScriptError(String),
    #[error("Invalid response from Redis: {0}")]
    InvalidResponse(String),
    #[error("Stream not found")]
    StreamNotFound,
    #[error("Invalid be_mult value")]
    InvalidBeMult,
    #[error("Missing required fields")]
    MissingRequiredFields,
    #[error("Invalid max retries")]
    InvalidMaxRetries,
    #[error("Invalid timeout seconds")]
    InvalidTimeoutSecs,
    #[error("Invalid reserved capacity")]
    InvalidReserved,
    #[error("Invalid prerequisites")]
    InvalidPrerequisites,
    #[error("JSON serialization error: {0}")]
    JsonError(#[from] serde_json::Error),
}

pub struct RedisClient {
    connection: Connection,
    scripts: HashMap<String, Script>,
}

impl RedisClient {
    pub async fn new(redis_url: &str) -> Result<Self, RedisErr> {
        let client = Client::open(redis_url)?;
        let connection = client.get_async_connection().await?;

        let mut redis_client = Self { connection, scripts: HashMap::new() };

        redis_client.load_scripts().await?;
        Ok(redis_client)
    }

    async fn load_scripts(&mut self) -> Result<(), RedisErr> {
        let script_contents = [
            ("create_job", include_str!("../lua/create_job.lua")),
            ("create_stream", include_str!("../lua/create_stream.lua")),
            ("create_task", include_str!("../lua/create_task.lua")),
            ("request_work", include_str!("../lua/request_work.lua")),
            ("update_task_done", include_str!("../lua/update_task_done.lua")),
            ("update_task_failed", include_str!("../lua/update_task_failed.lua")),
            ("update_task_retry", include_str!("../lua/update_task_retry.lua")),
        ];

        for (name, content) in script_contents {
            let script = Script::new(content);
            self.scripts.insert(name.to_string(), script);
        }

        Ok(())
    }

    pub async fn create_stream(&mut self, worker_type: &str) -> Result<String, RedisErr> {
        let script = self
            .scripts
            .get("create_stream")
            .ok_or_else(|| RedisErr::ScriptError("create_stream script not found".to_string()))?;

        let result: RedisResult<String> =
            script.arg(worker_type).invoke_async(&mut self.connection).await;

        let response = result?;
        // Try to parse as JSON to check for errors
        if let Ok(value) = serde_json::from_str::<Value>(&response)
            && let Some(obj) = value.as_object()
            && let Some(err) = obj.get("err").and_then(|v| v.as_str())
        {
            match err {
                "MissingRequiredFields" => return Err(RedisErr::MissingRequiredFields),
                _ => return Err(RedisErr::ScriptError(format!("Script error: {}", err))),
            }
        }
        Ok(response)
    }

    pub async fn create_job(
        &mut self,
        stream_id: &str,
        task_def: &Value,
        max_retries: u32,
        timeout_secs: u32,
        user_id: &str,
    ) -> Result<String, RedisErr> {
        let script = self
            .scripts
            .get("create_job")
            .ok_or_else(|| RedisErr::ScriptError("create_job script not found".to_string()))?;

        let result: RedisResult<String> = script
            .arg(stream_id)
            .arg(serde_json::to_string(task_def)?)
            .arg(max_retries)
            .arg(timeout_secs)
            .arg(user_id)
            .invoke_async(&mut self.connection)
            .await;

        let response = result?;
        // Try to parse as JSON to check for errors
        if let Ok(value) = serde_json::from_str::<Value>(&response)
            && let Some(obj) = value.as_object()
            && let Some(err) = obj.get("err").and_then(|v| v.as_str())
        {
            match err {
                "StreamNotFound" => return Err(RedisErr::StreamNotFound),
                "MissingRequiredFields" => return Err(RedisErr::MissingRequiredFields),
                "InvalidMaxRetries" => return Err(RedisErr::InvalidMaxRetries),
                "InvalidTimeoutSecs" => return Err(RedisErr::InvalidTimeoutSecs),
                _ => return Err(RedisErr::ScriptError(format!("Script error: {}", err))),
            }
        }
        Ok(response)
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn create_task(
        &mut self,
        job_id: &str,
        task_id: &str,
        stream_id: &str,
        task_def: &Value,
        prerequisites: &[String],
        max_retries: u32,
        timeout_secs: u32,
    ) -> Result<(), RedisErr> {
        let script = self
            .scripts
            .get("create_task")
            .ok_or_else(|| RedisErr::ScriptError("create_task script not found".to_string()))?;

        let result: RedisResult<String> = script
            .arg(job_id)
            .arg(task_id)
            .arg(stream_id)
            .arg(serde_json::to_string(task_def)?)
            .arg(serde_json::to_string(prerequisites)?)
            .arg(max_retries)
            .arg(timeout_secs)
            .invoke_async(&mut self.connection)
            .await;

        let response = result?;

        // Check for error responses
        if let Ok(value) = serde_json::from_str::<Value>(&response)
            && let Some(obj) = value.as_object()
            && let Some(err) = obj.get("err").and_then(|v| v.as_str())
        {
            match err {
                "MissingRequiredFields" => return Err(RedisErr::MissingRequiredFields),
                "InvalidMaxRetries" => return Err(RedisErr::InvalidMaxRetries),
                "InvalidTimeoutSecs" => return Err(RedisErr::InvalidTimeoutSecs),
                "InvalidPrerequisites" => return Err(RedisErr::InvalidPrerequisites),
                "StreamNotFound" => return Err(RedisErr::StreamNotFound),
                _ => return Err(RedisErr::ScriptError(format!("Script error: {}", err))),
            }
        }

        if response == "OK" {
            Ok(())
        } else {
            Err(RedisErr::InvalidResponse("Unexpected response from create_task".to_string()))
        }
    }

    pub async fn request_work(&mut self, worker_type: &str) -> Result<Option<TaskWork>, RedisErr> {
        let script = self
            .scripts
            .get("request_work")
            .ok_or_else(|| RedisErr::ScriptError("request_work script not found".to_string()))?;

        let result: RedisResult<Vec<String>> =
            script.arg(worker_type).invoke_async(&mut self.connection).await;

        match result? {
            arr if arr.len() == 5 => {
                let job_id = &arr[0];
                let task_id = &arr[1];
                let task_def_str = &arr[2];
                let prerequisites_str = &arr[3];
                let max_retries_str = &arr[4];

                let task_def: Value = serde_json::from_str(task_def_str)?;
                let prereqs: Vec<String> = serde_json::from_str(prerequisites_str)?;
                let max_retries: u32 = max_retries_str
                    .parse()
                    .map_err(|_| RedisErr::InvalidResponse("Invalid max_retries".to_string()))?;

                Ok(Some(TaskWork {
                    job_id: job_id.clone(),
                    task_id: task_id.clone(),
                    task_def,
                    prerequisites: prereqs,
                    max_retries,
                }))
            }
            _ => Ok(None), // No work available
        }
    }

    pub async fn update_task_done(
        &mut self,
        job_id: &str,
        task_id: &str,
        output: &Value,
    ) -> Result<bool, RedisErr> {
        let script = self.scripts.get("update_task_done").ok_or_else(|| {
            RedisErr::ScriptError("update_task_done script not found".to_string())
        })?;

        let result: RedisResult<String> = script
            .arg(job_id)
            .arg(task_id)
            .arg(serde_json::to_string(output)?)
            .invoke_async(&mut self.connection)
            .await;

        let response = result?;
        Ok(response == "1")
    }

    pub async fn update_task_failed(
        &mut self,
        job_id: &str,
        task_id: &str,
        error: &str,
    ) -> Result<bool, RedisErr> {
        let script = self.scripts.get("update_task_failed").ok_or_else(|| {
            RedisErr::ScriptError("update_task_failed script not found".to_string())
        })?;

        let result: RedisResult<String> =
            script.arg(job_id).arg(task_id).arg(error).invoke_async(&mut self.connection).await;

        let response = result?;
        Ok(response == "1")
    }

    pub async fn update_task_retry(
        &mut self,
        job_id: &str,
        task_id: &str,
    ) -> Result<bool, RedisErr> {
        let script = self.scripts.get("update_task_retry").ok_or_else(|| {
            RedisErr::ScriptError("update_task_retry script not found".to_string())
        })?;

        let result: RedisResult<String> =
            script.arg(job_id).arg(task_id).invoke_async(&mut self.connection).await;

        let response = result?;
        Ok(response == "1")
    }
}

#[derive(Debug, Clone)]
pub struct TaskWork {
    pub job_id: String,
    pub task_id: String,
    pub task_def: Value,
    pub prerequisites: Vec<String>,
    pub max_retries: u32,
}
