// Copyright 2026 Boundless Foundation, Inc.
//
// Use of this source code is governed by the Business Source License
// as found in the LICENSE-BSL file.

use anyhow::{Context, Result};
pub use deadpool_redis::{Config, Pool as RedisPool, Runtime, redis::AsyncCommands};
use redis::{FromRedisValue, RedisResult, ToRedisArgs};
use std::time::Instant;
use workflow_common::metrics::helpers;

pub fn create_pool(redis_url: &str) -> Result<RedisPool> {
    Config::from_url(redis_url)
        .create_pool(Some(Runtime::Tokio1))
        .context("Failed to init redis pool")
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
