// Copyright 2026 Boundless Foundation, Inc.
//
// Use of this source code is governed by the Business Source License
// as found in the LICENSE-BSL file.

use anyhow::{Context, Result, ensure};
use serde_json::json;
use taskdb::{JobState, redis_backend::RedisTaskDb};
use uuid::Uuid;

#[tokio::main]
async fn main() -> Result<()> {
    let redis_url = std::env::var("REDIS_URL")
        .unwrap_or_else(|_| "redis://default:valkey@127.0.0.1:6379".to_string());
    let namespace = std::env::var("TASKDB_NAMESPACE")
        .unwrap_or_else(|_| format!("taskdb-redis-e2e-{}", Uuid::new_v4()));

    let db = RedisTaskDb::with_namespace(&redis_url, &namespace)
        .with_context(|| format!("failed to create RedisTaskDb for url {redis_url}"))?;

    db.ping().await.context("failed to connect to Redis")?;
    db.flush_namespace().await.context("failed to clean namespace")?;

    println!("using namespace: {namespace}");

    let user_id = "e2e-user";
    let stream_id =
        db.create_stream("CPU", 1, 1.0, user_id).await.context("create_stream failed")?;

    let job_id = db
        .create_job(&stream_id, &json!({"init": "redis_e2e"}), 1, 5, user_id)
        .await
        .context("create_job failed")?;

    db.create_task(
        &job_id,
        "segment_1",
        &stream_id,
        &json!({"kind": "segment", "idx": 1}),
        &json!([]),
        1,
        5,
    )
    .await
    .context("create_task segment_1 failed")?;

    db.create_task(
        &job_id,
        "segment_2",
        &stream_id,
        &json!({"kind": "segment", "idx": 2}),
        &json!([]),
        1,
        5,
    )
    .await
    .context("create_task segment_2 failed")?;

    db.create_task(
        &job_id,
        "join_3",
        &stream_id,
        &json!({"kind": "join", "left": "segment_1", "right": "segment_2"}),
        &json!(["segment_1", "segment_2"]),
        1,
        5,
    )
    .await
    .context("create_task join_3 failed")?;

    println!("job created: {job_id}");

    let mut did_retry = false;
    let mut completed = 0usize;

    loop {
        let Some(task) = db.request_work("CPU").await.context("request_work failed")? else {
            break;
        };

        println!("claimed task: {}:{}", task.job_id, task.task_id);

        if task.task_id == "segment_2" && !did_retry {
            let requeued = db
                .update_task_retry(&task.job_id, &task.task_id)
                .await
                .context("update_task_retry failed")?;
            ensure!(requeued, "segment_2 was not retried as expected");
            println!("retried task once: {}:{}", task.job_id, task.task_id);
            did_retry = true;
            continue;
        }

        let updated = db
            .update_task_done(
                &task.job_id,
                &task.task_id,
                json!({"ok": true, "task_id": task.task_id}),
            )
            .await
            .context("update_task_done failed")?;

        ensure!(updated, "task unexpectedly was not updated");
        completed += 1;
    }

    ensure!(did_retry, "example did not exercise retry path");
    ensure!(completed >= 4, "expected at least 4 completed tasks, got {completed}");

    let state = db.get_job_state(&job_id, user_id).await.context("get_job_state failed")?;
    ensure!(matches!(state, JobState::Done), "job should be done, got {state}");

    let unresolved = db.get_job_unresolved(&job_id).await.context("get_job_unresolved failed")?;
    ensure!(unresolved == 0, "job has unresolved tasks: {unresolved}");

    let output: serde_json::Value =
        db.get_task_output(&job_id, "join_3").await.context("get_task_output join_3 failed")?;

    ensure!(output.get("ok").and_then(|v| v.as_bool()) == Some(true), "unexpected join output");

    let running = db.get_concurrent_jobs(user_id).await.context("get_concurrent_jobs failed")?;
    ensure!(running == 0, "expected no running jobs, got {running}");

    db.delete_job(&job_id).await.context("delete_job failed")?;

    println!("redis taskdb e2e passed for job: {job_id}");

    Ok(())
}
