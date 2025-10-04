// Copyright 2025 RISC Zero, Inc.
//
// Use of this source code is governed by the Business Source License
// as found in the LICENSE-BSL file.

use anyhow::Result;
use serde_json::Value as JsonValue;
use taskdb_redis::{
    JobState, ReadyTask, RedisTaskDB, TaskState,
    planner::{
        Planner,
        task::{Command as TaskCmd, Task},
    },
    test_helpers,
};
use uuid::Uuid;

#[tokio::test]
async fn e2e() -> Result<()> {
    let redis_url = "redis://127.0.0.1:6379";
    let mut db = RedisTaskDB::new(redis_url).await?;

    // Clean up any existing data
    test_helpers::cleanup(&mut db).await?;

    let image_id = "41414141";
    let input_id = "42424242";
    let aux_worker_type = "AUX";
    let cpu_worker_type = "CPU";
    let gpu_worker_type = "GPU";
    let segments = 3;
    let user_id = "user1";

    // Create streams
    let aux_stream = db.create_stream(aux_worker_type, 1, 1.0, user_id).await?;
    let cpu_stream = db.create_stream(cpu_worker_type, 1, 1.0, user_id).await?;
    let gpu_stream = db.create_stream(gpu_worker_type, 1, 1.0, user_id).await?;

    // Create a new workflow
    let task_def = serde_json::json!({"image_id": image_id, "input_id": input_id});
    let job_id = db.create_job(&aux_stream, &task_def, 0, 100, user_id).await?;

    // Init stage
    let task = db.request_work(aux_worker_type).await?.unwrap();
    assert_eq!(task.task_id, "init");

    // Forward to executor
    db.create_task(&job_id, "executor", &cpu_stream, &task.task_def, &serde_json::json!([]), 0, 10)
        .await?;

    db.update_task_done(&job_id, &task.task_id, JsonValue::default()).await?;

    // Executor stage
    let exec_task = db.request_work(cpu_worker_type).await?.unwrap();
    assert_eq!(exec_task.task_def.get("image_id").unwrap().as_str().unwrap(), image_id);
    assert_eq!(exec_task.task_def.get("input_id").unwrap().as_str().unwrap(), input_id);

    // Mock executor - create planner and tasks
    let mut planner = Planner::default();
    for _ in 0..segments {
        planner.enqueue_segment().unwrap();
    }
    planner.finish().unwrap();

    // Process planner tasks
    while let Some(tree_task) = planner.next_task() {
        process_task(&mut db, &tree_task, &exec_task, &gpu_stream, &aux_stream).await?;
    }

    // Complete executor
    db.update_task_done(&job_id, &exec_task.task_id, JsonValue::default()).await?;

    // Process GPU tasks
    for i in 0..segments {
        let task = db.request_work(gpu_worker_type).await?.unwrap();
        assert_eq!(task.task_id, i.to_string());

        // Mock GPU work
        let output = serde_json::json!({"segment": i, "proof": "mock_proof"});
        db.update_task_done(&job_id, &task.task_id, output).await?;
    }

    // Check job completion
    let job_state = db.get_job_state(&job_id, user_id).await?;
    assert_eq!(job_state, JobState::Done);

    // Cleanup
    test_helpers::cleanup(&mut db).await?;

    Ok(())
}

async fn process_task(
    db: &mut RedisTaskDB,
    tree_task: &Task,
    db_task: &ReadyTask,
    gpu_stream: &Uuid,
    aux_stream: &Uuid,
) -> Result<()> {
    match tree_task.command {
        TaskCmd::Keccak => {
            let task_def = serde_json::json!({"Keccak": { "segment": tree_task.task_number }});
            let prereqs = serde_json::json!([]);
            let task_name = format!("{}", tree_task.task_number);

            db.create_task(&db_task.job_id, &task_name, gpu_stream, &task_def, &prereqs, 0, 10)
                .await?;
        }
        TaskCmd::Segment => {
            let task_def = serde_json::json!({"Prove": { "index": tree_task.task_number }});
            let prereqs = serde_json::json!([]);
            let task_name = format!("{}", tree_task.task_number);

            db.create_task(&db_task.job_id, &task_name, gpu_stream, &task_def, &prereqs, 0, 10)
                .await?;
        }
        TaskCmd::Join => {
            let task_def = serde_json::json!({
                "Join": {
                    "idx": tree_task.task_number,
                    "left": tree_task.depends_on[0],
                    "right": tree_task.depends_on[1]
                }
            });
            let prereqs = serde_json::json!([
                tree_task.depends_on[0].to_string(),
                tree_task.depends_on[1].to_string()
            ]);
            let task_name = format!("{}", tree_task.task_number);

            db.create_task(&db_task.job_id, &task_name, gpu_stream, &task_def, &prereqs, 0, 10)
                .await?;
        }
        TaskCmd::Union => {
            let task_def = serde_json::json!({
                "Union": {
                    "idx": tree_task.task_number,
                    "left": tree_task.keccak_depends_on[0],
                    "right": tree_task.keccak_depends_on[1]
                }
            });
            let prereqs = serde_json::json!([
                tree_task.keccak_depends_on[0].to_string(),
                tree_task.keccak_depends_on[1].to_string()
            ]);
            let task_name = format!("{}", tree_task.task_number);

            db.create_task(&db_task.job_id, &task_name, gpu_stream, &task_def, &prereqs, 0, 10)
                .await?;
        }
        TaskCmd::Finalize => {
            let task_def = serde_json::json!({
                "Finalize": {
                    "max_idx": tree_task.depends_on[0]
                }
            });
            let prereqs = serde_json::json!([tree_task.depends_on[0].to_string()]);
            let task_name = format!("{}", tree_task.task_number);

            db.create_task(&db_task.job_id, &task_name, aux_stream, &task_def, &prereqs, 0, 10)
                .await?;
        }
    }

    Ok(())
}

#[tokio::test]
async fn test_stream_prioritization() -> Result<()> {
    let redis_url = "redis://127.0.0.1:6379";
    let mut db = RedisTaskDB::new(redis_url).await?;

    test_helpers::cleanup(&mut db).await?;

    let user_id = "user1";
    let worker_type = "CPU";

    // Create streams with different priorities
    let stream_0 = db.create_stream(worker_type, 0, 1.0, user_id).await?;
    let stream_1 = db.create_stream(worker_type, 1, 1.0, user_id).await?;

    // Create jobs
    let job_0 = db.create_job(&stream_0, &JsonValue::default(), 0, 100, user_id).await?;
    let job_1 = db.create_job(&stream_1, &JsonValue::default(), 0, 100, user_id).await?;

    // Add tasks to both streams
    db.create_task(
        &job_0,
        "task0",
        &stream_0,
        &JsonValue::default(),
        &serde_json::json!([]),
        0,
        10,
    )
    .await?;
    db.create_task(
        &job_1,
        "task1",
        &stream_1,
        &JsonValue::default(),
        &serde_json::json!([]),
        0,
        10,
    )
    .await?;

    // Complete init tasks
    let init_0 = db.request_work(worker_type).await?.unwrap();
    db.update_task_done(&job_0, &init_0.task_id, JsonValue::default()).await?;

    let init_1 = db.request_work(worker_type).await?.unwrap();
    db.update_task_done(&job_1, &init_1.task_id, JsonValue::default()).await?;

    // Request work - should get from stream_1 first (has reservation)
    let task = db.request_work(worker_type).await?.unwrap();
    assert_eq!(task.job_id, job_1);

    // Complete task_1
    db.update_task_done(&job_1, &task.task_id, JsonValue::default()).await?;

    // Next task should be from stream_0
    let task = db.request_work(worker_type).await?.unwrap();
    assert_eq!(task.job_id, job_0);

    test_helpers::cleanup(&mut db).await?;
    Ok(())
}

#[tokio::test]
async fn test_task_dependencies() -> Result<()> {
    let redis_url = "redis://127.0.0.1:6379";
    let mut db = RedisTaskDB::new(redis_url).await?;

    test_helpers::cleanup(&mut db).await?;

    let user_id = "user1";
    let worker_type = "CPU";
    let stream_id = db.create_stream(worker_type, 1, 1.0, user_id).await?;
    let job_id = db.create_job(&stream_id, &JsonValue::default(), 0, 100, user_id).await?;

    // Complete init task
    let init_task = db.request_work(worker_type).await?.unwrap();
    db.update_task_done(&job_id, &init_task.task_id, JsonValue::default()).await?;

    // Create dependent tasks
    db.create_task(
        &job_id,
        "task1",
        &stream_id,
        &JsonValue::default(),
        &serde_json::json!([]),
        0,
        10,
    )
    .await?;
    db.create_task(
        &job_id,
        "task2",
        &stream_id,
        &JsonValue::default(),
        &serde_json::json!([]),
        0,
        10,
    )
    .await?;
    db.create_task(
        &job_id,
        "join",
        &stream_id,
        &JsonValue::default(),
        &serde_json::json!(["task1", "task2"]),
        0,
        10,
    )
    .await?;

    // Process tasks
    let task1 = db.request_work(worker_type).await?.unwrap();
    assert_eq!(task1.task_id, "task1");
    db.update_task_done(&job_id, &task1.task_id, JsonValue::default()).await?;

    let task2 = db.request_work(worker_type).await?.unwrap();
    assert_eq!(task2.task_id, "task2");
    db.update_task_done(&job_id, &task2.task_id, JsonValue::default()).await?;

    // Now join task should be ready
    let join_task = db.request_work(worker_type).await?.unwrap();
    assert_eq!(join_task.task_id, "join");

    test_helpers::cleanup(&mut db).await?;
    Ok(())
}
