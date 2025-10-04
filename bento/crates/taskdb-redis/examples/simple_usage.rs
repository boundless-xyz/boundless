// Copyright 2025 RISC Zero, Inc.
//
// Use of this source code is governed by the Business Source License
// as found in the LICENSE-BSL file.

use anyhow::Result;
use serde_json::Value as JsonValue;
use taskdb_redis::{JobState, RedisTaskDB, TaskState};
use uuid::Uuid;

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize Redis TaskDB
    let redis_url = "redis://127.0.0.1:6379";
    let mut db = RedisTaskDB::new(redis_url).await?;

    println!("🚀 Starting Redis TaskDB example...");

    // Clean up any existing data
    taskdb_redis::test_helpers::cleanup(&mut db).await?;

    // Create streams for different worker types
    let cpu_stream = db.create_stream("cpu", 2, 1.0, "user1").await?;
    let gpu_stream = db.create_stream("gpu", 1, 2.0, "user1").await?;

    println!("✅ Created streams: CPU (reserved: 2), GPU (reserved: 1, multiplier: 2.0)");

    // Create a job
    let job_data = serde_json::json!({
        "image_id": "41414141",
        "input_id": "42424242",
        "segments": 4
    });

    let job_id = db.create_job(&cpu_stream, &job_data, 3, 300, "user1").await?;
    println!("✅ Created job: {}", job_id);

    // Process the init task
    let init_task = db.request_work("cpu").await?.unwrap();
    println!("📋 Processing init task: {}", init_task.task_id);

    // Create some tasks that depend on init
    db.create_task(
        &job_id,
        "segment_0",
        &gpu_stream,
        &serde_json::json!({"type": "prove", "index": 0}),
        &serde_json::json!(["init"]),
        2,
        60,
    )
    .await?;

    db.create_task(
        &job_id,
        "segment_1",
        &gpu_stream,
        &serde_json::json!({"type": "prove", "index": 1}),
        &serde_json::json!(["init"]),
        2,
        60,
    )
    .await?;

    db.create_task(
        &job_id,
        "join_0",
        &gpu_stream,
        &serde_json::json!({"type": "join", "left": 0, "right": 1}),
        &serde_json::json!(["segment_0", "segment_1"]),
        2,
        30,
    )
    .await?;

    println!("✅ Created 3 tasks: 2 segments + 1 join");

    // Complete init task
    db.update_task_done(&job_id, &init_task.task_id, JsonValue::default()).await?;
    println!("✅ Completed init task");

    // Process GPU tasks
    let mut completed_tasks = 0;
    while completed_tasks < 3 {
        if let Some(task) = db.request_work("gpu").await? {
            println!(
                "🔄 Processing {} task: {}",
                task.task_def.get("type").unwrap().as_str().unwrap(),
                task.task_id
            );

            // Simulate work
            tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

            // Complete task
            let output = serde_json::json!({
                "result": "success",
                "proof": format!("proof_{}", task.task_id),
                "duration_ms": 100
            });

            db.update_task_done(&job_id, &task.task_id, output).await?;
            completed_tasks += 1;
            println!("✅ Completed task: {}", task.task_id);
        } else {
            // No work available, wait a bit
            tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
        }
    }

    // Check final job state
    let job_state = db.get_job_state(&job_id, "user1").await?;
    println!("🎉 Job completed with state: {:?}", job_state);

    // Show some statistics
    let unresolved = db.get_job_unresolved(&job_id).await?;
    let job_time = db.get_job_time(&job_id).await?;
    let concurrent_jobs = db.get_concurrent_jobs("user1").await?;

    println!("📊 Statistics:");
    println!("   - Unresolved tasks: {}", unresolved);
    println!("   - Job execution time: {:.2}s", job_time);
    println!("   - Concurrent jobs for user1: {}", concurrent_jobs);

    // Demonstrate error handling
    println!("\n🔍 Testing error handling...");

    // Create a job that will fail
    let fail_job_id = db.create_job(&cpu_stream, &JsonValue::default(), 1, 1, "user1").await?;
    let fail_task = db.request_work("cpu").await?.unwrap();

    // Simulate task failure
    db.update_task_failed(&fail_job_id, &fail_task.task_id, "Simulated failure").await?;

    let fail_job_state = db.get_job_state(&fail_job_id, "user1").await?;
    let failure_reason = db.get_job_failure(&fail_job_id).await?;

    println!("❌ Failed job state: {:?}", fail_job_state);
    println!("❌ Failure reason: {}", failure_reason);

    // Demonstrate retry mechanism
    println!("\n🔄 Testing retry mechanism...");

    let retry_job_id = db.create_job(&cpu_stream, &JsonValue::default(), 2, 1, "user1").await?;
    let retry_task = db.request_work("cpu").await?.unwrap();

    // Fail the task
    db.update_task_failed(&retry_job_id, &retry_task.task_id, "Temporary failure").await?;

    // Retry the task
    let retry_success = db.update_task_retry(&retry_job_id, &retry_task.task_id).await?;
    println!("🔄 Retry successful: {}", retry_success);

    // Complete the retried task
    if let Some(retry_task) = db.request_work("cpu").await? {
        db.update_task_done(&retry_job_id, &retry_task.task_id, JsonValue::default()).await?;
        println!("✅ Retried task completed successfully");
    }

    // Clean up
    taskdb_redis::test_helpers::cleanup(&mut db).await?;
    println!("🧹 Cleaned up test data");

    println!("\n🎉 Redis TaskDB example completed successfully!");
    println!("Key benefits demonstrated:");
    println!("  ✅ Drop-in replacement for PostgreSQL TaskDB");
    println!("  ✅ Same API - no code changes needed");
    println!("  ✅ Better performance with Redis operations");
    println!("  ✅ Atomic operations with Lua scripts");
    println!("  ✅ Built-in data structures for task queues");
    println!("  ✅ Simpler architecture - one system instead of two");

    Ok(())
}
