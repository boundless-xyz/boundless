// Copyright (c) 2024 RISC Zero, Inc.
//
// All rights reserved.

use std::str::FromStr;

use anyhow::{bail, Context, Result};
use clap::Parser;
use rand::{rngs::StdRng, seq::SliceRandom, Rng, SeedableRng};
use sqlx::{
    postgres::PgPoolOptions,
    types::{JsonValue, Uuid},
    PgPool,
};
use taskdb::{
    planner::{
        task::{Command as TaskCmd, Task},
        Planner,
    },
    test_helpers, update_task_done, update_task_failed, update_task_retry, ReadyTask,
};
use tokio::{
    task::JoinSet,
    time::{sleep, Duration},
};
use tracing_subscriber::filter::EnvFilter;

async fn create_customer(pool: &PgPool, customer_id: u64) -> Result<(Uuid, Uuid)> {
    let customer_id_str = format!("user_{customer_id}");
    let cpu_stream = taskdb::create_stream(pool, "CPU", 0, 1.0, &customer_id_str)
        .await
        .context("Failed to create cpu stream")?;
    let gpu_stream = taskdb::create_stream(pool, "GPU", 0, 1.0, &customer_id_str)
        .await
        .context("Failed to create cpu stream")?;

    Ok((cpu_stream, gpu_stream))
}

async fn spawner(
    pool: PgPool,
    customer_count: u64,
    seed: u64,
    job_delay: u64,
    max_job_size: u64,
) -> Result<()> {
    tracing::info!("Starting spawner..");
    let mut customers = vec![];
    for idx in 0..customer_count {
        customers
            .push((create_customer(&pool, idx).await.context("Failed to create customer")?, idx));
    }
    let mut r = StdRng::seed_from_u64(seed);

    loop {
        // Pick a random
        let &((cpu_stream, gpu_stream), user_idx) = customers.choose(&mut r).unwrap();
        let segment_count = r.gen_range(1..max_job_size);
        let user_id = format!("user_{user_idx}");
        let task_def = serde_json::json!({
            "cpu_stream": gpu_stream.to_string(),
            "gpu_stream": gpu_stream.to_string(),
            "segments": segment_count,
        });
        let job_id = taskdb::create_job(&pool, &cpu_stream, &task_def, 0, 100, &user_id)
            .await
            .context("Failed to create job")?;
        tracing::info!("Spawning new job: {job_id} segments; {segment_count}");

        sleep(Duration::from_secs(job_delay)).await;
    }
}

// TODO: Deduplicate this code from the e2e?
async fn process_task(pool: &PgPool, tree_task: &Task, db_task: &ReadyTask) -> Result<()> {
    let cpu_stream = db_task.task_def.get("cpu_stream").unwrap().as_str().unwrap();
    let cpu_stream = Uuid::from_str(cpu_stream).unwrap();
    let gpu_stream = db_task.task_def.get("gpu_stream").unwrap().as_str().unwrap();
    let gpu_stream = Uuid::from_str(gpu_stream).unwrap();

    match tree_task.command {
        TaskCmd::Segment => {
            let task_def = serde_json::json!({"Prove": { "segment": tree_task.task_number }});
            let prereqs = serde_json::json!([]);
            let task_name = format!("{}", tree_task.task_number);

            // println!("inserting: segment {}", task_name);
            taskdb::create_task(
                pool,
                &db_task.job_id,
                &task_name,
                &gpu_stream,
                &task_def,
                &prereqs,
                2,
                60,
            )
            .await
            .unwrap();
        }
        TaskCmd::Join => {
            let task_def = serde_json::json!({
                "Join": {
                    "left": tree_task.depends_on[0],
                    "right": tree_task.depends_on[1],
                    "index": tree_task.task_number
                }
            });
            let prereqs = serde_json::json!([
                format!("{}", tree_task.depends_on[0]),
                format!("{}", tree_task.depends_on[1])
            ]);
            let task_name = format!("{}", tree_task.task_number);

            // println!("inserting join {} - {:?}", task_name, prereqs);
            taskdb::create_task(
                pool,
                &db_task.job_id,
                &task_name,
                &gpu_stream,
                &task_def,
                &prereqs,
                2,
                2,
            )
            .await
            .unwrap();
        }
        TaskCmd::Finalize => {
            let task_def = serde_json::json!({
                "Finalize": {
                }
            });
            let prereqs = serde_json::json!([format!("{}", tree_task.depends_on[0])]);

            // println!("inserting finalize {} {:?}", tree_task.task_number, prereqs);
            taskdb::create_task(
                pool,
                &db_task.job_id,
                "finalize",
                &cpu_stream,
                &task_def,
                &prereqs,
                0,
                10,
            )
            .await
            .unwrap();
        }
    }
    Ok(())
}

async fn run_exec_task(pool: &PgPool, task: &ReadyTask) -> Result<()> {
    let mut planner = Planner::default();
    let segments = task
        .task_def
        .get("segments")
        .context("task missing segments json elm")?
        .as_u64()
        .context("segments not u64")?;

    tracing::info!("Executor running job: {}", task.job_id);
    for _ in 0..segments {
        planner.enqueue_segment().unwrap();

        while let Some(tree_task) = planner.next_task() {
            process_task(pool, tree_task, task).await.unwrap();
        }
    }

    planner.finish().unwrap();
    while let Some(tree_task) = planner.next_task() {
        process_task(pool, tree_task, task).await.unwrap();
    }
    tracing::info!("Executor completed job: {}", task.job_id);

    Ok(())
}

async fn worker(
    pool: PgPool,
    worker_id: u32,
    worker_type: &str,
    seed: u64,
    work_delay: u64,
) -> Result<()> {
    tracing::info!("in worker: {worker_type} idx: {worker_id}");
    let mut r = StdRng::seed_from_u64(seed + worker_id as u64);

    loop {
        match taskdb::request_work(&pool, worker_type).await {
            Ok(task) => {
                let Some(task) = task else {
                    continue;
                };

                match worker_type {
                    "CPU" => {
                        match task.task_id.as_str() {
                            taskdb::INIT_TASK => {
                                run_exec_task(&pool, &task).await.context("failed to run exec")?
                            }
                            "finalize" => {}
                            _ => bail!("unsupported CPU task"),
                        }
                        let res = update_task_done(
                            &pool,
                            &task.job_id,
                            &task.task_id,
                            JsonValue::default(),
                        )
                        .await
                        .context("Failed to set task to complete")?;
                        if !res {
                            tracing::error!(
                                "Failed to update task to done: {}:{}",
                                task.job_id,
                                task.task_id
                            );
                        }
                    }
                    "GPU" => {
                        // wait random ms in range
                        let seconds = r.gen_range(1..work_delay);
                        tracing::info!(
                            "[{worker_id}] GPU running for {seconds} ms, job: {}:{}",
                            task.job_id,
                            task.task_id
                        );
                        sleep(Duration::from_millis(seconds)).await;

                        // 1 in N chance to fail a task
                        if r.gen_range(0..100) == 0 {
                            tracing::error!("Intentionally failing job: {}", task.job_id);
                            if !update_task_failed(&pool, &task.job_id, &task.task_id, "ERROR")
                                .await
                                .context("Failed to update_task_failed")?
                            {
                                tracing::error!(
                                    "Failed to fail task {}:{}, probably already failing..",
                                    task.job_id,
                                    task.task_id
                                );
                            }
                        } else if r.gen_range(0..200) == 0 {
                            tracing::warn!("Retrying task {}:{}", task.job_id, task.task_id);
                            if !update_task_retry(&pool, &task.job_id, &task.task_id)
                                .await
                                .context("Failed to task_retry")?
                            {
                                tracing::error!(
                                    "Failed to retry task {}:{}, probably already failing..",
                                    task.job_id,
                                    task.task_id
                                );
                            }
                        } else {
                            let res = update_task_done(
                                &pool,
                                &task.job_id,
                                &task.task_id,
                                JsonValue::default(),
                            )
                            .await
                            .context("Failed to set task to complete")?;

                            // tracing::info!(
                            //     "Completed GPU task: {}:{} - res: {}",
                            //     task.job_id,
                            //     task.task_id,
                            //     res
                            // );
                            if !res {
                                tracing::error!(
                                    "Failed to update task to done: {}:{}",
                                    task.job_id,
                                    task.task_id
                                );
                            }
                        }
                    }
                    _ => bail!("Unsupported work type"),
                }
            }
            Err(err) => bail!(err),
        }
    }
}

/// TaskDB Stress testing harness
#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// cpu workers
    #[arg(short, long, default_value = "1")]
    cpu_workers: u32,

    /// gpu workers
    #[arg(short, long, default_value = "4")]
    gpu_workers: u32,

    /// Customers with work
    #[arg(short = 's', long, default_value = "1")]
    customers: u64,

    /// Time between starting new jobs, in seconds
    #[arg(short, long, default_value = "5")]
    job_speed: u64,

    /// Max size of new jobs, in segments
    #[arg(short, long, default_value = "32")]
    max_job_size: u64,

    /// Range (in ms) to use for the GPU work
    #[arg(short = 't', long, default_value = "800")]
    gpu_work_speed: u64,

    /// RNG seed
    #[arg(short, long, default_value = "41")]
    rng_seed: u64,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt().with_env_filter(EnvFilter::from_default_env()).init();

    let args = Args::parse();
    let conn_url = std::env::var("DATABASE_URL").expect("Env var DATABASE_URL is required.");
    let db = PgPoolOptions::new().connect(&conn_url).await.unwrap();

    let mut tasks = JoinSet::new();
    let pool = db.clone();
    tasks.spawn(spawner(pool, args.customers, args.rng_seed, args.job_speed, args.max_job_size));

    // Create reaper
    let pool_copy = db.clone();
    tasks.spawn(async move {
        loop {
            let requeued_tasks =
                taskdb::requeue_tasks(&pool_copy, 100).await.expect("Failed to requeue tasks");
            if requeued_tasks > 0 {
                tracing::warn!("requeued {requeued_tasks} tasks");
            }

            sleep(Duration::from_secs(2)).await;
        }
    });

    for i in 0..args.cpu_workers {
        let pool_copy = db.clone();
        tasks.spawn(worker(pool_copy, i, "CPU", args.rng_seed, args.gpu_work_speed));
    }
    for i in 0..args.gpu_workers {
        let pool_copy = db.clone();
        tasks.spawn(worker(pool_copy, i, "GPU", args.rng_seed, args.gpu_work_speed));
    }

    // TODO: Fix graceful shutdowns

    // ctrl-c handler
    // let db_copy = db.clone();
    // tokio::spawn(async move {
    //     tokio::signal::ctrl_c()
    //         .await
    //         .expect("Failed to register ctrl+c handler");
    //     tracing::warn!("Graceful shutdown, cleaning up...");
    //     test_helpers::cleanup(&db_copy).await;
    //     panic!("DONE");
    // });

    while let Some(res) = tasks.join_next().await {
        res.unwrap().unwrap();
    }

    tracing::info!("Wrapping up work...");
    test_helpers::cleanup(&db).await;

    Ok(())
}
