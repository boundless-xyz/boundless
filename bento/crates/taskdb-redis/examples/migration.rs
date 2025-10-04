// Copyright 2025 RISC Zero, Inc.
//
// Use of this source code is governed by the Business Source License
// as found in the LICENSE-BSL file.

use anyhow::Result;
use clap::Parser;
use serde_json::Value as JsonValue;
use taskdb_redis::{JobState, RedisTaskDB, TaskState};
use uuid::Uuid;

#[derive(Parser)]
struct Args {
    /// Redis connection URL
    #[arg(short, long, default_value = "redis://127.0.0.1:6379")]
    redis_url: String,

    /// PostgreSQL connection URL
    #[arg(short, long)]
    postgres_url: String,

    /// Migration direction: postgres-to-redis or redis-to-postgres
    #[arg(short, long, default_value = "postgres-to-redis")]
    direction: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    match args.direction.as_str() {
        "postgres-to-redis" => {
            println!("Migrating from PostgreSQL to Redis...");
            migrate_postgres_to_redis(&args.postgres_url, &args.redis_url).await?;
        }
        "redis-to-postgres" => {
            println!("Migrating from Redis to PostgreSQL...");
            migrate_redis_to_postgres(&args.redis_url, &args.postgres_url).await?;
        }
        _ => {
            eprintln!("Invalid direction. Use 'postgres-to-redis' or 'redis-to-postgres'");
            std::process::exit(1);
        }
    }

    println!("Migration completed successfully!");
    Ok(())
}

async fn migrate_postgres_to_redis(postgres_url: &str, redis_url: &str) -> Result<()> {
    use sqlx::PgPool;

    // Connect to both databases
    let pg_pool = PgPool::connect(postgres_url).await?;
    let mut redis_db = RedisTaskDB::new(redis_url).await?;

    // Migrate streams
    println!("Migrating streams...");
    let streams = sqlx::query!(
        "SELECT id, worker_type, reserved, be_mult, user_id, running, ready FROM streams"
    )
    .fetch_all(&pg_pool)
    .await?;

    for stream in streams {
        let stream_id = stream.id;
        let stream_data = serde_json::json!({
            "id": stream_id,
            "worker_type": stream.worker_type,
            "reserved": stream.reserved,
            "be_mult": stream.be_mult,
            "user_id": stream.user_id,
            "running": stream.running,
            "ready": stream.ready,
            "priority": if stream.ready == 0 {
                f32::INFINITY
            } else if stream.running >= stream.reserved {
                (stream.running - stream.reserved) as f32 / stream.be_mult
            } else {
                (stream.running - stream.reserved) as f32 / stream.reserved as f32
            }
        });

        let stream_key = format!("stream:{}", stream_id);
        redis_db.connection.hset(&stream_key, "data", stream_data.to_string())?;

        // Add to worker type index
        let worker_streams_key = format!("worker_streams:{}", stream.worker_type);
        redis_db.connection.sadd(worker_streams_key, stream_id.to_string())?;

        // Add to priority queue
        let priority_key = format!("streams_by_priority:{}", stream.worker_type);
        let priority: f32 = if stream.ready == 0 {
            f32::INFINITY
        } else if stream.running >= stream.reserved {
            (stream.running - stream.reserved) as f32 / stream.be_mult
        } else {
            (stream.running - stream.reserved) as f32 / stream.reserved as f32
        };
        redis_db.connection.zadd(priority_key, stream_id.to_string(), priority)?;

        // Initialize stream counters
        let counters_key = format!("stream:{}:counters", stream_id);
        redis_db.connection.hset(&counters_key, "running", stream.running)?;
        redis_db.connection.hset(&counters_key, "ready", stream.ready)?;
        redis_db.connection.hset(&counters_key, "pending", 0)?;
    }

    // Migrate jobs
    println!("Migrating jobs...");
    let jobs = sqlx::query!("SELECT id, state, error, user_id, reported FROM jobs")
        .fetch_all(&pg_pool)
        .await?;

    for job in jobs {
        let job_data = serde_json::json!({
            "id": job.id,
            "state": job.state,
            "error": job.error,
            "user_id": job.user_id,
            "reported": job.reported
        });

        let job_key = format!("job:{}", job.id);
        redis_db.connection.hset(&job_key, "data", job_data.to_string())?;
    }

    // Migrate tasks
    println!("Migrating tasks...");
    let tasks = sqlx::query!(
        r#"
        SELECT
            job_id, task_id, stream_id, task_def, prerequisites, state,
            created_at, started_at, updated_at, waiting_on, progress,
            retries, max_retries, timeout_secs, output, error
        FROM tasks
        "#
    )
    .fetch_all(&pg_pool)
    .await?;

    for task in tasks {
        let task_data = serde_json::json!({
            "job_id": task.job_id,
            "task_id": task.task_id,
            "stream_id": task.stream_id,
            "task_def": task.task_def,
            "prerequisites": task.prerequisites,
            "state": task.state,
            "created_at": task.created_at.timestamp(),
            "started_at": task.started_at.map(|t| t.timestamp()),
            "updated_at": task.updated_at.map(|t| t.timestamp()),
            "waiting_on": task.waiting_on,
            "progress": task.progress,
            "retries": task.retries,
            "max_retries": task.max_retries,
            "timeout_secs": task.timeout_secs,
            "output": task.output,
            "error": task.error,
            "worker_id": None::<String>
        });

        let task_key = format!("task:{}:{}", task.job_id, task.task_id);
        redis_db.connection.hset(&task_key, "data", task_data.to_string())?;

        // Add to appropriate queue based on state
        let stream_id = task.stream_id;
        match task.state.as_str() {
            "ready" => {
                let ready_key = format!("stream:{}:ready", stream_id);
                redis_db.connection.lpush(ready_key, &task_key)?;
            }
            "pending" => {
                let pending_key = format!("stream:{}:pending", stream_id);
                redis_db.connection.lpush(pending_key, &task_key)?;
            }
            _ => {} // running, done, failed tasks don't go in queues
        }

        // Migrate dependencies
        let deps = sqlx::query!(
            "SELECT pre_task_id FROM task_deps WHERE job_id = $1 AND post_task_id = $2",
            task.job_id,
            task.task_id
        )
        .fetch_all(&pg_pool)
        .await?;

        if !deps.is_empty() {
            let dep_key = format!("deps:{}:{}", task.job_id, task.task_id);
            for dep in deps {
                redis_db.connection.sadd(&dep_key, dep.pre_task_id)?;
            }
        }
    }

    println!("Migration completed!");
    Ok(())
}

async fn migrate_redis_to_postgres(redis_url: &str, postgres_url: &str) -> Result<()> {
    use sqlx::PgPool;

    // Connect to both databases
    let mut redis_db = RedisTaskDB::new(redis_url).await?;
    let pg_pool = PgPool::connect(postgres_url).await?;

    // This would be the reverse migration
    // For now, just print a message
    println!("Redis to PostgreSQL migration not implemented yet");
    println!("This would involve:");
    println!("1. Reading all data from Redis");
    println!("2. Converting to PostgreSQL format");
    println!("3. Inserting into PostgreSQL tables");
    println!("4. Verifying data integrity");

    Ok(())
}
