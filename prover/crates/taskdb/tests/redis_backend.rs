use taskdb::{INIT_TASK, JobState, Priority, TaskDbErr, redis_backend::RedisTaskDb};
use uuid::Uuid;

async fn test_db() -> Option<RedisTaskDb> {
    let redis_url = std::env::var("REDIS_URL").unwrap_or_else(|_| "redis://127.0.0.1/".to_string());
    let ns = format!("taskdb-redis-test-{}", Uuid::new_v4());
    let db = RedisTaskDb::with_namespace(&redis_url, ns).ok()?;
    if db.ping().await.is_err() {
        eprintln!("Skipping Redis taskdb test; REDIS_URL is unreachable");
        return None;
    }

    db.flush_namespace().await.ok()?;
    Some(db)
}

#[tokio::test]
async fn request_work_and_dependency_unblock() -> Result<(), TaskDbErr> {
    let Some(db) = test_db().await else {
        return Ok(());
    };

    let stream_id = db.create_stream("CPU", 1, 1.0, "user-a").await?;
    let job_id =
        db.create_job(&stream_id, &serde_json::json!({"init": "test"}), 2, 30, "user-a").await?;

    let init = db.request_work("CPU").await?.expect("expected init task");
    assert_eq!(init.task_id, INIT_TASK);
    assert!(db.update_task_done(&job_id, INIT_TASK, serde_json::json!({"ok": true})).await?);

    db.create_task(
        &job_id,
        "a",
        &stream_id,
        &serde_json::json!({"task": "a"}),
        &serde_json::json!([]),
        2,
        30,
    )
    .await?;

    db.create_task(
        &job_id,
        "b",
        &stream_id,
        &serde_json::json!({"task": "b"}),
        &serde_json::json!([]),
        2,
        30,
    )
    .await?;

    db.create_task(
        &job_id,
        "join",
        &stream_id,
        &serde_json::json!({"task": "join"}),
        &serde_json::json!(["a", "b"]),
        2,
        30,
    )
    .await?;

    let first = db.request_work("CPU").await?.expect("expected first child task");
    let second = db.request_work("CPU").await?.expect("expected second child task");
    assert_ne!(first.task_id, second.task_id);
    assert!(matches!(first.task_id.as_str(), "a" | "b"));
    assert!(matches!(second.task_id.as_str(), "a" | "b"));

    db.update_task_done(&job_id, &first.task_id, serde_json::json!({"done": 1})).await?;
    assert!(db.request_work("CPU").await?.is_none());

    db.update_task_done(&job_id, &second.task_id, serde_json::json!({"done": 1})).await?;

    let join = db.request_work("CPU").await?.expect("expected join task");
    assert_eq!(join.task_id, "join");

    db.update_task_done(&job_id, &join.task_id, serde_json::json!({"joined": true})).await?;

    let state = db.get_job_state(&job_id, "user-a").await?;
    assert_eq!(state, JobState::Done);

    Ok(())
}

#[tokio::test]
async fn timeout_requeue_and_retry_cap() -> Result<(), TaskDbErr> {
    let Some(db) = test_db().await else {
        return Ok(());
    };

    let stream_id = db.create_stream("CPU", 1, 1.0, "user-b").await?;
    let job_id =
        db.create_job(&stream_id, &serde_json::json!({"init": "test"}), 1, 1, "user-b").await?;

    let init = db.request_work("CPU").await?.expect("expected init task");
    assert_eq!(init.task_id, INIT_TASK);

    tokio::time::sleep(tokio::time::Duration::from_millis(1200)).await;

    let requeued = db.requeue_tasks(100).await?;
    assert_eq!(requeued, 1);

    let retried = db.request_work("CPU").await?.expect("expected retried task");
    assert_eq!(retried.task_id, INIT_TASK);

    let can_retry = db.update_task_retry(&job_id, INIT_TASK).await?;
    assert!(!can_retry);

    let state = db.get_job_state(&job_id, "user-b").await?;
    assert_eq!(state, JobState::Failed);

    Ok(())
}

#[tokio::test]
async fn request_work_prefers_priority_over_stream_ordering() -> Result<(), TaskDbErr> {
    let Some(db) = test_db().await else {
        return Ok(());
    };

    let low_stream = db.create_stream("GPU", 1, 1.0, "user-c").await?;
    let high_stream = db.create_stream("GPU", 2, 1.0, "user-d").await?;

    let job_low = db
        .create_job_with_priority(
            &low_stream,
            &serde_json::json!({"init": "low"}),
            0,
            30,
            "user-c",
            Priority::Low,
        )
        .await?;
    let job_high = db
        .create_job_with_priority(
            &high_stream,
            &serde_json::json!({"init": "high"}),
            0,
            30,
            "user-d",
            Priority::High,
        )
        .await?;

    let first = db.request_work("GPU").await?.expect("expected first task");
    assert_eq!(first.job_id, job_high);

    let second = db.request_work("GPU").await?.expect("expected second task");
    assert_eq!(second.job_id, job_low);

    Ok(())
}

#[tokio::test]
async fn request_work_uses_fifo_within_same_priority() -> Result<(), TaskDbErr> {
    let Some(db) = test_db().await else {
        return Ok(());
    };

    let first_stream = db.create_stream("GPU", 1, 1.0, "user-fifo-a").await?;
    let second_stream = db.create_stream("GPU", 2, 1.0, "user-fifo-b").await?;

    let first_job = db
        .create_job_with_priority(
            &first_stream,
            &serde_json::json!({"init": "first"}),
            0,
            30,
            "user-fifo-a",
            Priority::Medium,
        )
        .await?;
    let second_job = db
        .create_job_with_priority(
            &second_stream,
            &serde_json::json!({"init": "second"}),
            0,
            30,
            "user-fifo-b",
            Priority::Medium,
        )
        .await?;

    let first = db.request_work("GPU").await?.expect("expected first medium task");
    let second = db.request_work("GPU").await?.expect("expected second medium task");

    assert_eq!(first.job_id, first_job);
    assert_eq!(second.job_id, second_job);

    Ok(())
}

#[tokio::test]
async fn next_claim_soft_preempts_lower_priority_work() -> Result<(), TaskDbErr> {
    let Some(db) = test_db().await else {
        return Ok(());
    };

    let low_stream = db.create_stream("CPU", 1, 1.0, "user-soft-low").await?;
    let high_stream = db.create_stream("CPU", 1, 1.0, "user-soft-high").await?;

    let low_job = db
        .create_job_with_priority(
            &low_stream,
            &serde_json::json!({"init": "low"}),
            0,
            30,
            "user-soft-low",
            Priority::Low,
        )
        .await?;

    let low_task = db.request_work("CPU").await?.expect("expected low-priority task");
    assert_eq!(low_task.job_id, low_job);

    let high_job = db
        .create_job_with_priority(
            &high_stream,
            &serde_json::json!({"init": "high"}),
            0,
            30,
            "user-soft-high",
            Priority::High,
        )
        .await?;

    assert!(db.update_task_done(&low_job, INIT_TASK, serde_json::json!({"done": true})).await?);

    let next =
        db.request_work("CPU").await?.expect("expected high-priority task after low finished");
    assert_eq!(next.job_id, high_job);

    Ok(())
}

#[tokio::test]
async fn child_tasks_inherit_job_priority() -> Result<(), TaskDbErr> {
    let Some(db) = test_db().await else {
        return Ok(());
    };

    let high_stream = db.create_stream("CPU", 1, 1.0, "user-child-high").await?;
    let low_stream = db.create_stream("CPU", 1, 1.0, "user-child-low").await?;

    let high_job = db
        .create_job_with_priority(
            &high_stream,
            &serde_json::json!({"init": "high"}),
            0,
            30,
            "user-child-high",
            Priority::High,
        )
        .await?;
    let high_init = db.request_work("CPU").await?.expect("expected high init task");
    assert_eq!(high_init.job_id, high_job);
    assert!(db.update_task_done(&high_job, INIT_TASK, serde_json::json!({"ok": true})).await?);

    db.create_task(
        &high_job,
        "child",
        &high_stream,
        &serde_json::json!({"task": "child"}),
        &serde_json::json!([]),
        0,
        30,
    )
    .await?;

    let low_job = db
        .create_job_with_priority(
            &low_stream,
            &serde_json::json!({"init": "low"}),
            0,
            30,
            "user-child-low",
            Priority::Low,
        )
        .await?;

    let next = db.request_work("CPU").await?.expect("expected inherited high-priority child task");
    assert_eq!(next.job_id, high_job);
    assert_eq!(next.task_id, "child");

    let low = db.request_work("CPU").await?.expect("expected low task after inherited high task");
    assert_eq!(low.job_id, low_job);

    Ok(())
}

#[tokio::test]
async fn request_work_blocking_wakes_for_new_job() -> Result<(), TaskDbErr> {
    let Some(db) = test_db().await else {
        return Ok(());
    };

    let stream_id = db.create_stream("CPU", 1, 1.0, "user-wake").await?;
    let db_clone = db.clone();

    let create_job = tokio::spawn(async move {
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
        db_clone
            .create_job(&stream_id, &serde_json::json!({"init": "wake"}), 0, 30, "user-wake")
            .await
    });

    let started = tokio::time::Instant::now();
    let task = db
        .request_work_blocking("CPU", 5)
        .await?
        .expect("expected blocking claim to wake for new job");
    let job_id = create_job.await.expect("job creation task panicked")?;

    assert_eq!(task.job_id, job_id);
    assert_eq!(task.task_id, INIT_TASK);
    assert!(started.elapsed() < tokio::time::Duration::from_secs(2));

    Ok(())
}

#[tokio::test]
async fn request_work_blocking_wakes_for_dependency_unblock() -> Result<(), TaskDbErr> {
    let Some(db) = test_db().await else {
        return Ok(());
    };

    let stream_id = db.create_stream("CPU", 1, 1.0, "user-unblock").await?;
    let job_id = db
        .create_job(&stream_id, &serde_json::json!({"init": "test"}), 0, 30, "user-unblock")
        .await?;

    let init = db.request_work("CPU").await?.expect("expected init task");
    assert_eq!(init.task_id, INIT_TASK);
    assert!(db.update_task_done(&job_id, INIT_TASK, serde_json::json!({"ok": true})).await?);

    db.create_task(
        &job_id,
        "a",
        &stream_id,
        &serde_json::json!({"task": "a"}),
        &serde_json::json!([]),
        0,
        30,
    )
    .await?;
    db.create_task(
        &job_id,
        "b",
        &stream_id,
        &serde_json::json!({"task": "b"}),
        &serde_json::json!([]),
        0,
        30,
    )
    .await?;
    db.create_task(
        &job_id,
        "join",
        &stream_id,
        &serde_json::json!({"task": "join"}),
        &serde_json::json!(["a", "b"]),
        0,
        30,
    )
    .await?;

    let first = db.request_work("CPU").await?.expect("expected first child task");
    let second = db.request_work("CPU").await?.expect("expected second child task");
    assert!(db.request_work("CPU").await?.is_none());

    db.update_task_done(&job_id, &first.task_id, serde_json::json!({"done": true})).await?;
    assert!(db.request_work("CPU").await?.is_none());

    let db_clone = db.clone();
    let job_id_clone = job_id;
    let second_task_id = second.task_id.clone();
    let finish_task = tokio::spawn(async move {
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
        db_clone
            .update_task_done(&job_id_clone, &second_task_id, serde_json::json!({"done": true}))
            .await
    });

    let join = db
        .request_work_blocking("CPU", 5)
        .await?
        .expect("expected blocking claim to wake for dependency unblock");
    finish_task.await.expect("dependency completion task panicked")?;

    assert_eq!(join.task_id, "join");

    Ok(())
}

#[tokio::test]
async fn request_work_blocking_times_out_when_no_work_arrives() -> Result<(), TaskDbErr> {
    let Some(db) = test_db().await else {
        return Ok(());
    };

    db.create_stream("CPU", 1, 1.0, "user-timeout").await?;

    let started = tokio::time::Instant::now();
    let task = db.request_work_blocking("CPU", 1).await?;

    assert!(task.is_none());
    assert!(started.elapsed() >= tokio::time::Duration::from_millis(900));
    assert!(started.elapsed() < tokio::time::Duration::from_secs(3));

    Ok(())
}

#[tokio::test]
async fn request_work_blocking_ignores_stale_notifications() -> Result<(), TaskDbErr> {
    let Some(db) = test_db().await else {
        return Ok(());
    };

    let stream_id = db.create_stream("CPU", 1, 1.0, "user-stale").await?;
    let first_job = db
        .create_job(&stream_id, &serde_json::json!({"init": "first"}), 0, 30, "user-stale")
        .await?;

    let init = db.request_work("CPU").await?.expect("expected first init task");
    assert_eq!(init.job_id, first_job);
    assert!(db.update_task_done(&first_job, INIT_TASK, serde_json::json!({"ok": true})).await?);

    let db_clone = db.clone();
    let second_stream_id = stream_id;
    let create_job = tokio::spawn(async move {
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
        db_clone
            .create_job(
                &second_stream_id,
                &serde_json::json!({"init": "second"}),
                0,
                30,
                "user-stale",
            )
            .await
    });

    let started = tokio::time::Instant::now();
    let task = db
        .request_work_blocking("CPU", 5)
        .await?
        .expect("expected blocking claim to wait past stale notification");
    let second_job = create_job.await.expect("second job creation task panicked")?;

    assert_eq!(task.job_id, second_job);
    assert_eq!(task.task_id, INIT_TASK);
    assert!(started.elapsed() >= tokio::time::Duration::from_millis(75));

    Ok(())
}
