// Copyright (c) 2024 RISC Zero, Inc.
//
// All rights reserved.

use criterion::{criterion_group, criterion_main, Criterion};
use sqlx::{postgres::PgPoolOptions, types::Uuid, PgPool};
use tokio::runtime::Runtime;

fn tokio() -> Runtime {
    Runtime::new().unwrap()
}

async fn create_run_task(pool: &PgPool, worker_type: &str, job_id: &Uuid, stream_id: &Uuid) {
    let json = serde_json::json!([]);
    taskdb::create_task(pool, job_id, "test", stream_id, &json, &json, 0, 1).await.unwrap();
    let task = taskdb::request_work(pool, worker_type).await.unwrap().unwrap();
    taskdb::update_task_done(pool, &task.job_id, &task.task_id, serde_json::json!({}))
        .await
        .unwrap();
}

fn criterion_benchmark(c: &mut Criterion) {
    let runtime = tokio();
    let database_url = std::env::var("DATABASE_URL").expect("DATABASE_URL must be set");
    let db = runtime
        .block_on(PgPoolOptions::new().max_connections(4).connect(&database_url))
        .expect("failed to connect to DATABASE_URL");

    c.bench_function("create_run_task", |b| {
        b.to_async(&runtime).iter(|| async {
            let worker_type = "CPU";
            let user_id = "user1";
            let stream_id = taskdb::create_stream(&db, worker_type, 1, 1.0, user_id).await.unwrap();
            let job_id = taskdb::create_job(&db, &stream_id, &serde_json::json!({}), 0, 1, user_id)
                .await
                .unwrap();
            create_run_task(&db, worker_type, &job_id, &stream_id).await;
        })
    });

    runtime.block_on(taskdb::test_helpers::cleanup(&db));
}

criterion_group!(benches, criterion_benchmark);
criterion_main!(benches);
