// Copyright 2025 RISC Zero, Inc.
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
    match ttl {
        Some(expiry) => {
            let redis_start = Instant::now();
            let result = conn.set_ex(key, value, expiry).await;
            let status = if result.is_ok() { "success" } else { "error" };
            helpers::record_redis_operation("set_ex", status, redis_start.elapsed().as_secs_f64());
            result
        }
        None => {
            let redis_start = Instant::now();
            let result = conn.set(key, value).await;
            let status = if result.is_ok() { "success" } else { "error" };
            helpers::record_redis_operation("set", status, redis_start.elapsed().as_secs_f64());
            result
        }
    }
}

pub async fn get_key<T>(conn: &mut deadpool_redis::Connection, key: &str) -> RedisResult<T>
where
    T: FromRedisValue + Send + Sync + 'static,
{
    let redis_start = Instant::now();
    let result = conn.get::<_, T>(key).await;
    let status = if result.is_ok() { "success" } else { "error" };
    helpers::record_redis_operation("get", status, redis_start.elapsed().as_secs_f64());
    result
}
