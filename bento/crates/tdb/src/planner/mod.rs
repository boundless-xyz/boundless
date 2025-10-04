//! # Task Planner
//!
//! The task planner provides sophisticated task dependency management and orchestration
//! for distributed task execution. It manages task peaks, keccak peaks, and creates
//! a dependency graph that can be executed by workers.
//!
//! ## Key Concepts
//!
//! - **Peaks**: Tasks that are either Segment or Join commands with no other join tasks depending on them
//! - **Keccak Peaks**: Tasks that are either Keccak Segment or Union commands with no other union tasks depending on them
//! - **Task Height**: The depth of a task in the dependency tree
//! - **Dependencies**: Tasks that must complete before another task can start
//!
//! ## Usage
//!
//! ```rust,no_run
//! use tdb::planner::Planner;
//!
//! #[tokio::main]
//! async fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     let mut planner = Planner::new_with_stream(
//!         "redis://127.0.0.1:6379",
//!         "worker_type",
//!         1,
//!         1.0,
//!         "user_id".to_string(),
//!     ).await?;
//!
//!     // Add tasks
//!     planner.enqueue_segment().await?;
//!     planner.enqueue_keccak().await?;
//!
//!     // Finish planning
//!     planner.finish().await?;
//!
//!     Ok(())
//! }
//! ```

// Copyright 2025 RISC Zero, Inc.
//
// Use of this source code is governed by the Business Source License
// as found in the LICENSE-BSL file.

pub mod task;

use crate::planner::task::Task;
use crate::redis_client::{RedisClient, RedisErr, TaskWork};
use serde_json::Value;
use std::{cmp::Ordering, collections::VecDeque};
use thiserror::Error;

#[derive(Error, Debug)]
pub enum PlannerErr {
    #[error("Planning not yet started")]
    PlanNotStartedString,
    #[error("Cannot add segment to finished plan")]
    PlanFinalized,
    #[error("Redis error: {0}")]
    RedisError(#[from] RedisErr),
    #[error("JSON serialization error: {0}")]
    JsonError(#[from] serde_json::Error),
}

pub struct Planner {
    /// Redis client for task management
    redis_client: RedisClient,

    /// Job ID for this planning session
    job_id: String,

    /// Stream ID for this planning session
    stream_id: String,

    /// User ID for this planning session
    user_id: String,

    /// All of the tasks in this plan (for planning logic)
    tasks: Vec<Task>,

    /// List of current "peaks." Sorted in order of decreasing height.
    ///
    /// A task is a "peak" if (1) it is either a Segment or Join command AND (2) no other join
    /// tasks depend on it.
    peaks: Vec<usize>,

    /// List of current "keccak_peaks." Sorted in order of decreasing height.
    ///
    /// A task is a "keccak_peak" if (1) it is either a Keccak Segment or Union command AND (2) no other union tasks depend on it.
    keccak_peaks: VecDeque<usize>,

    /// Iterator position. Used by `self.next_task()`.
    consumer_position: usize,

    /// Last task in the plan. Set by `self.finish()`.
    last_task: Option<usize>,

    /// Whether the plan has been finalized
    finalized: bool,
}

impl core::fmt::Debug for Planner {
    fn fmt(&self, f: &mut core::fmt::Formatter) -> core::fmt::Result {
        use crate::planner::task::Command;

        let mut stack = Vec::new();

        if self.last_task.is_none() {
            writeln!(f, "Still in planning phases ...")?;
        } else {
            stack.push((0, self.last_task.unwrap()));
        }

        while let Some((indent, cursor)) = stack.pop() {
            if indent > 0 {
                write!(f, "\n{}", " ".repeat(indent))?
            }

            let task = self.get_task(cursor);

            match task.command {
                Command::Finalize => {
                    write!(f, "{:?} Finalize", task.task_number)?;
                    stack.push((indent + 2, task.depends_on[0]));
                }
                Command::Join => {
                    write!(f, "{:?} Join", task.task_number)?;
                    stack.push((indent + 2, task.depends_on[0]));
                    stack.push((indent + 2, task.depends_on[1]));
                }
                Command::Segment => {
                    write!(f, "{:?} Segment", task.task_number)?;
                }
                Command::Keccak => {
                    write!(f, "{:?} Keccak", task.task_number)?;
                }
                Command::Union => {
                    write!(f, "{:?} Union", task.task_number)?;
                    stack.push((indent + 2, task.depends_on[0]));
                    stack.push((indent + 2, task.depends_on[1]));
                }
            }
        }

        Ok(())
    }
}

impl Planner {
    /// Create a new planner with Redis backend
    pub async fn new(
        redis_url: &str,
        stream_id: String,
        user_id: String,
    ) -> Result<Self, PlannerErr> {
        let mut redis_client = RedisClient::new(redis_url).await?;

        // Create a job for this planning session
        let job_id = redis_client
            .create_job(
                &stream_id,
                &Value::Object(serde_json::Map::new()),
                3,   // max_retries
                300, // timeout_secs
                &user_id,
            )
            .await?;

        Ok(Planner {
            redis_client,
            job_id,
            stream_id,
            user_id,
            tasks: Vec::new(),
            peaks: Vec::new(),
            keccak_peaks: VecDeque::new(),
            consumer_position: 0,
            last_task: None,
            finalized: false,
        })
    }

    /// Create a new planner with a new stream
    pub async fn new_with_stream(
        redis_url: &str,
        worker_type: &str,
        reserved: u32,
        be_mult: f64,
        user_id: String,
    ) -> Result<Self, PlannerErr> {
        let mut redis_client = RedisClient::new(redis_url).await?;

        // Create a stream first
        let stream_id =
            redis_client.create_stream(worker_type, reserved, be_mult, &user_id).await?;

        // Create a job for this planning session
        let job_id = redis_client
            .create_job(
                &stream_id,
                &Value::Object(serde_json::Map::new()),
                3,   // max_retries
                300, // timeout_secs
                &user_id,
            )
            .await?;

        Ok(Planner {
            redis_client,
            job_id,
            stream_id,
            user_id,
            tasks: Vec::new(),
            peaks: Vec::new(),
            keccak_peaks: VecDeque::new(),
            consumer_position: 0,
            last_task: None,
            finalized: false,
        })
    }
    pub async fn enqueue_segment(&mut self) -> Result<usize, PlannerErr> {
        if self.finalized {
            return Err(PlannerErr::PlanFinalized);
        }

        let task_number = self.next_task_number();
        let task = Task::new_segment(task_number);
        self.tasks.push(task);

        // Create task in Redis
        let task_def = Value::Object(serde_json::Map::new());
        let task_id = format!("segment_{}", task_number);
        self.redis_client
            .create_task(
                &self.job_id,
                &task_id,
                &self.stream_id,
                &task_def,
                &[],
                3,   // max_retries
                300, // timeout_secs
            )
            .await?;

        let mut new_peak = task_number;
        while let Some(smallest_peak) = self.peaks.last().copied() {
            let new_height = self.get_task(new_peak).task_height;
            let smallest_peak_height = self.get_task(smallest_peak).task_height;

            match new_height.cmp(&smallest_peak_height) {
                Ordering::Less => break,
                Ordering::Equal => {
                    self.peaks.pop();
                    new_peak = self.enqueue_join(smallest_peak, new_peak).await?;
                }
                Ordering::Greater => unreachable!(),
            }
        }
        self.peaks.push(new_peak);

        Ok(task_number)
    }

    pub async fn enqueue_keccak(&mut self) -> Result<usize, PlannerErr> {
        if self.finalized {
            return Err(PlannerErr::PlanFinalized);
        }

        let task_number = self.next_task_number();
        let task = Task::new_keccak(task_number);
        self.tasks.push(task);

        // Create task in Redis
        let task_def = Value::Object(serde_json::Map::new());
        let task_id = format!("keccak_{}", task_number);
        self.redis_client
            .create_task(
                &self.job_id,
                &task_id,
                &self.stream_id,
                &task_def,
                &[],
                3,   // max_retries
                300, // timeout_secs
            )
            .await?;

        let mut new_peak = task_number;
        while let Some(smallest_peak) = self.keccak_peaks.back().copied() {
            let new_height = self.get_task(new_peak).task_height;
            let smallest_peak_height = self.get_task(smallest_peak).task_height;
            match new_height.cmp(&smallest_peak_height) {
                Ordering::Less => break,
                Ordering::Equal => {
                    self.keccak_peaks.pop_back();
                    new_peak = self.enqueue_union(smallest_peak, new_peak).await?;
                }
                Ordering::Greater => unreachable!(),
            }
        }
        self.keccak_peaks.push_back(new_peak);

        Ok(task_number)
    }

    async fn finish_unions(&mut self) -> Result<Option<VecDeque<usize>>, PlannerErr> {
        // this assumes that there's always a minimum of 1 segment
        if self.keccak_peaks.is_empty() {
            return Ok(None);
        }

        // Join remaining peaks if using union
        while 2 <= self.keccak_peaks.len() {
            let peak_0 = self.keccak_peaks.pop_front().unwrap();
            let peak_1 = self.keccak_peaks.pop_front().unwrap();

            let peak_3 = self.enqueue_union(peak_1, peak_0).await?;
            self.keccak_peaks.push_front(peak_3);
        }

        // return only highest peak
        Ok(Some(VecDeque::from([self.keccak_peaks[0]])))
    }

    pub async fn finish(&mut self) -> Result<usize, PlannerErr> {
        // Return error if plan has not yet started
        if self.peaks.is_empty() {
            return Err(PlannerErr::PlanNotStartedString);
        }

        // finish unions
        let keccak_depends_on = self.finish_unions().await?;

        // Finish the plan (if it's not yet finished)
        if self.last_task.is_none() {
            // Join remaining peaks
            while 2 <= self.peaks.len() {
                let peak_0 = self.peaks.pop().unwrap();
                let peak_1 = self.peaks.pop().unwrap();

                let peak_3 = self.enqueue_join(peak_1, peak_0).await?;
                self.peaks.push(peak_3);
            }

            // Add the Finalize task
            self.last_task = Some(self.enqueue_finalize(self.peaks[0], keccak_depends_on).await?);
        }

        self.finalized = true;
        Ok(self.last_task.unwrap())
    }

    pub fn task_count(&self) -> usize {
        self.tasks.len()
    }

    pub fn get_task(&self, task_number: usize) -> &Task {
        if task_number < self.tasks.len() {
            &self.tasks[task_number]
        } else {
            panic!("Invalid task number {task_number}");
        }
    }

    pub fn next_task(&mut self) -> Option<&Task> {
        if self.consumer_position < self.task_count() {
            let out = &self.tasks[self.consumer_position];
            self.consumer_position += 1;
            Some(out)
        } else {
            None
        }
    }

    async fn enqueue_join(&mut self, left: usize, right: usize) -> Result<usize, PlannerErr> {
        let task_number = self.next_task_number();
        let task_height =
            1 + u32::max(self.get_task(left).task_height, self.get_task(right).task_height);
        let task = Task::new_join(task_number, task_height, left, right);
        self.tasks.push(task);

        // Create task in Redis
        let task_def = Value::Object(serde_json::Map::new());
        let task_id = format!("join_{}", task_number);
        let left_task_id = format!("task_{}", left);
        let right_task_id = format!("task_{}", right);
        self.redis_client
            .create_task(
                &self.job_id,
                &task_id,
                &self.stream_id,
                &task_def,
                &[left_task_id, right_task_id],
                3,   // max_retries
                300, // timeout_secs
            )
            .await?;

        Ok(task_number)
    }

    async fn enqueue_union(&mut self, left: usize, right: usize) -> Result<usize, PlannerErr> {
        let task_number = self.next_task_number();
        let task_height =
            1 + u32::max(self.get_task(left).task_height, self.get_task(right).task_height);
        let task = Task::new_union(task_number, task_height, left, right);
        self.tasks.push(task);

        // Create task in Redis
        let task_def = Value::Object(serde_json::Map::new());
        let task_id = format!("union_{}", task_number);
        let left_task_id = format!("task_{}", left);
        let right_task_id = format!("task_{}", right);
        self.redis_client
            .create_task(
                &self.job_id,
                &task_id,
                &self.stream_id,
                &task_def,
                &[left_task_id, right_task_id],
                3,   // max_retries
                300, // timeout_secs
            )
            .await?;

        Ok(task_number)
    }

    async fn enqueue_finalize(
        &mut self,
        depends_on: usize,
        keccak_depends_on: Option<VecDeque<usize>>,
    ) -> Result<usize, PlannerErr> {
        let task_number = self.next_task_number();
        let mut task_height = 1 + self.get_task(depends_on).task_height;
        if let Some(val) = &keccak_depends_on
            && let Some(highest_peak) = val.iter().max().copied()
        {
            task_height = task_height.max(1 + self.get_task(highest_peak).task_height);
        }
        let task = Task::new_finalize(task_number, task_height, depends_on, keccak_depends_on);
        self.tasks.push(task);

        // Create task in Redis
        let task_def = Value::Object(serde_json::Map::new());
        let task_id = format!("finalize_{}", task_number);
        let depends_on_task_id = format!("task_{}", depends_on);
        self.redis_client
            .create_task(
                &self.job_id,
                &task_id,
                &self.stream_id,
                &task_def,
                &[depends_on_task_id],
                3,   // max_retries
                300, // timeout_secs
            )
            .await?;

        Ok(task_number)
    }

    fn next_task_number(&self) -> usize {
        self.task_count()
    }

    /// Request work from the Redis queue
    pub async fn request_work(
        &mut self,
        worker_type: &str,
    ) -> Result<Option<TaskWork>, PlannerErr> {
        self.redis_client.request_work(worker_type).await.map_err(Into::into)
    }

    /// Mark a task as done
    pub async fn update_task_done(
        &mut self,
        job_id: &str,
        task_id: &str,
        output: &Value,
    ) -> Result<bool, PlannerErr> {
        self.redis_client.update_task_done(job_id, task_id, output).await.map_err(Into::into)
    }

    /// Mark a task as failed
    pub async fn update_task_failed(
        &mut self,
        job_id: &str,
        task_id: &str,
        error: &str,
    ) -> Result<bool, PlannerErr> {
        self.redis_client.update_task_failed(job_id, task_id, error).await.map_err(Into::into)
    }

    /// Retry a task
    pub async fn update_task_retry(
        &mut self,
        job_id: &str,
        task_id: &str,
    ) -> Result<bool, PlannerErr> {
        self.redis_client.update_task_retry(job_id, task_id).await.map_err(Into::into)
    }

    /// Get the job ID for this planner
    pub fn job_id(&self) -> &str {
        &self.job_id
    }

    /// Get the stream ID for this planner
    pub fn stream_id(&self) -> &str {
        &self.stream_id
    }

    /// Get the user ID for this planner
    pub fn user_id(&self) -> &str {
        &self.user_id
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio;

    #[tokio::test]
    async fn test_simple_plan() {
        // Note: This test requires a Redis instance running
        // In a real test environment, you would use a test Redis instance
        let redis_url = "redis://127.0.0.1:6379";

        let mut planner =
            Planner::new_with_stream(redis_url, "test_worker", 1, 1.0, "test_user".to_string())
                .await
                .unwrap();

        let numb = planner.enqueue_segment().await.unwrap();
        assert_eq!(numb, 0);
        let task = planner.next_task().unwrap();
        assert_eq!(task.keccak_depends_on.len(), 0);
        assert_eq!(task.depends_on.len(), 0);
        assert_eq!(task.command, task::Command::Segment);
        assert_eq!(task.task_number, 0);
        assert!(planner.next_task().is_none());

        let numb = planner.enqueue_keccak().await.unwrap();
        let task = planner.next_task().unwrap();
        assert_eq!(numb, 1);
        assert_eq!(task.keccak_depends_on.len(), 0);
        assert_eq!(task.depends_on.len(), 0);
        assert_eq!(task.command, task::Command::Keccak);
        assert_eq!(task.task_number, 1);
        assert!(planner.next_task().is_none());

        planner.finish().await.unwrap();
        let task = planner.next_task().unwrap();
        assert_eq!(task.command, task::Command::Finalize);
        assert_eq!(task.keccak_depends_on.len(), 1);
        assert_eq!(task.task_number, 2);
        assert_eq!(task.depends_on.len(), 1);
        assert_eq!(task.task_height, 1);

        assert_eq!(planner.task_count(), 3);
        assert_eq!(planner.get_task(0).task_number, 0);
    }

    #[tokio::test]
    async fn test_balanced() {
        let redis_url = "redis://127.0.0.1:6379";

        let mut planner =
            Planner::new_with_stream(redis_url, "test_worker", 1, 1.0, "test_user".to_string())
                .await
                .unwrap();

        planner.enqueue_segment().await.unwrap();
        {
            let task = planner.next_task().unwrap();
            assert_eq!(task.command, task::Command::Segment);
            assert_eq!(task.task_number, 0);
            assert_eq!(task.task_height, 0);
            assert!(planner.next_task().is_none());
        }

        planner.enqueue_keccak().await.unwrap();
        {
            let task = planner.next_task().unwrap();
            assert_eq!(task.command, task::Command::Keccak);
            assert_eq!(task.task_number, 1);
            assert_eq!(task.task_height, 0);
        }

        planner.enqueue_segment().await.unwrap();
        {
            let task = planner.next_task().unwrap();
            assert_eq!(task.command, task::Command::Segment);
            assert_eq!(task.task_number, 2);
            assert_eq!(task.task_height, 0);
            assert_eq!(task.depends_on.len(), 0);
        }
        {
            let join = planner.next_task().unwrap();
            assert_eq!(join.command, task::Command::Join);
            assert_eq!(join.task_number, 3);
            assert_eq!(join.task_height, 1);
            assert_eq!(join.depends_on.len(), 2);
        }

        planner.enqueue_keccak().await.unwrap();
        {
            let task = planner.next_task().unwrap();
            assert_eq!(task.command, task::Command::Keccak);
            assert_eq!(task.task_number, 4);
            assert_eq!(task.task_height, 0);
        }
        {
            let union = planner.next_task().unwrap();
            assert_eq!(union.command, task::Command::Union);
            assert_eq!(union.task_number, 5);
            assert_eq!(union.task_height, 1);
            assert_eq!(union.keccak_depends_on.len(), 2);
        }

        planner.finish().await.unwrap();
        let keccak_last_task = planner.next_task().unwrap();
        assert_eq!(keccak_last_task.command, task::Command::Finalize);
        assert_eq!(keccak_last_task.task_number, 6);
        assert_eq!(keccak_last_task.task_height, 2);
        assert_eq!(keccak_last_task.depends_on.len(), 1);
        assert_eq!(keccak_last_task.keccak_depends_on.len(), 1);
    }

    #[tokio::test]
    async fn test_error_handling() {
        let redis_url = "redis://127.0.0.1:6379";

        let mut planner =
            Planner::new_with_stream(redis_url, "test_worker", 1, 1.0, "test_user".to_string())
                .await
                .unwrap();

        // Test that we can't add segments after finishing
        planner.enqueue_segment().await.unwrap();
        planner.finish().await.unwrap();

        let result = planner.enqueue_segment().await;
        assert!(matches!(result, Err(PlannerErr::PlanFinalized)));
    }
}
