// Copyright 2025 Boundless Foundation, Inc.
//
// Use of this source code is governed by the Business Source License
// as found in the LICENSE-BSL file.

use anyhow::{Context, Result};
pub use deadpool_redis::{Config, Pool as RedisPool, Runtime, redis::AsyncCommands};
use redis::{ErrorKind, FromRedisValue, RedisError, RedisResult, ToRedisArgs};
use std::time::Instant;
use workflow_common::metrics::helpers;

pub fn create_pool(redis_url: &str) -> Result<RedisPool> {
    Config::from_url(redis_url)
        .create_pool(Some(Runtime::Tokio1))
        .context("Failed to init redis pool")
}

/// Snapshot of redis memory usage as reported by `INFO memory`
#[derive(Debug, Clone, Copy)]
pub struct RedisMemoryInfo {
    /// Total heap reported by redis in bytes
    pub used_memory: u64,
    /// Configured redis `maxmemory` value (0 means unlimited)
    pub maxmemory: u64,
    /// Host memory as reported by redis (may reflect host, not cgroup)
    pub total_system_memory: Option<u64>,
}

/// Fetch redis memory statistics via `INFO memory` query to redis/valkey.
pub async fn fetch_memory_info(
    conn: &mut deadpool_redis::Connection,
) -> RedisResult<RedisMemoryInfo> {
    let redis_start = Instant::now();
    let info_result: RedisResult<String> = redis::cmd("INFO").arg("memory").query_async(conn).await;
    let elapsed = redis_start.elapsed().as_secs_f64();
    let status = if info_result.is_ok() { "success" } else { "error" };
    helpers::record_redis_operation("info_memory", status, elapsed);
    let info = info_result?;

    let mut used_memory = None;
    let mut maxmemory = None;
    let mut total_system_memory = None;

    for line in info.lines() {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some((key, value)) = line.split_once(':') {
            match key {
                "used_memory" => used_memory = Some(parse_memory_field("used_memory", value)?),
                "maxmemory" => maxmemory = Some(parse_memory_field("maxmemory", value)?),
                "total_system_memory" => {
                    total_system_memory = Some(parse_memory_field("total_system_memory", value)?)
                }
                _ => {}
            }
        }
    }

    let used_memory = used_memory.ok_or_else(|| missing_field_err("used_memory"))?;
    let maxmemory = maxmemory.unwrap_or(0);

    Ok(RedisMemoryInfo { used_memory, maxmemory, total_system_memory })
}

fn parse_memory_field(field: &str, value: &str) -> RedisResult<u64> {
    value.trim().parse::<u64>().map_err(|_| {
        RedisError::from((
            ErrorKind::TypeError,
            "Invalid INFO memory payload",
            format!("Failed to parse {field} from INFO memory"),
        ))
    })
}

fn missing_field_err(field: &str) -> RedisError {
    RedisError::from((
        ErrorKind::TypeError,
        "Incomplete INFO memory payload",
        format!("INFO memory missing {field} field"),
    ))
}

/// Function to set a key with an optional expiry using a pipeline
pub async fn set_key_with_expiry<T>(
    conn: &mut deadpool_redis::Connection,
    key: &str,
    value: T,
    ttl: Option<u64>,
) -> RedisResult<()>
where
    T: ToRedisArgs + Send + Sync + 'static,
{
    let redis_start = Instant::now();
    let (operation, result) = match ttl {
        Some(expiry) => {
            let result = conn.set_ex(key, value, expiry).await;
            ("set_ex", result)
        }
        None => {
            let result = conn.set(key, value).await;
            ("set", result)
        }
    };
    let elapsed = redis_start.elapsed().as_secs_f64();
    let status = if result.is_ok() { "success" } else { "error" };
    helpers::record_redis_operation(operation, status, elapsed);
    result
}

pub async fn get_key<T>(conn: &mut deadpool_redis::Connection, key: &str) -> RedisResult<T>
where
    T: FromRedisValue + Send + Sync + 'static,
{
    let redis_start = Instant::now();
    let result = conn.get::<_, T>(key).await;
    let elapsed = redis_start.elapsed().as_secs_f64();
    let status = if result.is_ok() { "success" } else { "error" };
    helpers::record_redis_operation("get", status, elapsed);
    result
}
