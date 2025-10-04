// Copyright 2025 RISC Zero, Inc.
//
// Use of this source code is governed by the Business Source License
// as found in the LICENSE-BSL file.

use anyhow::Result;
use serde_json::Value as JsonValue;
use taskdb_redis::{JobState, RedisTaskDB, TaskState};
use uuid::Uuid;

#[tokio::test]
async fn test_workflow_redis_integration() -> Result<()> {
    let redis_url = "redis://127.0.0.1:6379";
    let mut taskdb = RedisTaskDB::new(redis_url).await?;

    // Clean up any existing data
    taskdb_redis::test_helpers::cleanup(&mut taskdb).await?;

    // Create a stream
    let stream_id = taskdb.create_stream("test_worker", 1, 1.0, "test_user").await?;
    println!("Created stream: {}", stream_id);

    // Create a job
    let job_data = serde_json::json!({
        "test": "data"
    });
    let job_id = taskdb.create_job(&stream_id, &job_data, 0, 100, "test_user").await?;
    println!("Created job: {}", job_id);

    // Create a task
    let task_def = serde_json::json!({
        "type": "test",
        "data": "test_task"
    });
    taskdb
        .create_task(&job_id, "test_task", &stream_id, &task_def, &serde_json::json!([]), 0, 10)
        .await?;
    println!("Created task");

    // Request work
    let task = taskdb.request_work("test_worker").await?;
    assert!(task.is_some());
    let task = task.unwrap();
    println!("Got task: {}", task.task_id);

    // Complete the task
    let output = serde_json::json!({
        "result": "success"
    });
    taskdb.update_task_done(&job_id, &task.task_id, output).await?;
    println!("Completed task");

    // Check job state
    let job_state = taskdb.get_job_state(&job_id, "test_user").await?;
    assert_eq!(job_state, JobState::Done);
    println!("Job completed successfully");

    // Clean up
    taskdb_redis::test_helpers::cleanup(&mut taskdb).await?;

    Ok(())
}
