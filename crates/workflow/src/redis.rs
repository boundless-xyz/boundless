// Copyright 2024 RISC Zero, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use anyhow::{Context, Result};
use deadpool_redis::Connection;
pub use deadpool_redis::{redis::AsyncCommands, Config, Pool as RedisPool, Runtime};
use redis::{RedisResult, ToRedisArgs};

pub fn create_pool(redis_url: &str) -> Result<RedisPool> {
    Config::from_url(redis_url)
        .create_pool(Some(Runtime::Tokio1))
        .context("Failed to init redis pool")
}

// A utility function to get a Redis connection from the pool.
pub async fn get_connection(pool: &RedisPool) -> Result<deadpool_redis::Connection> {
    let conn = pool.get().await?;
    Ok(conn)
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
        Some(expiry) => conn.set_ex(key, value, expiry).await,
        None => conn.set(key, value).await,
    }
}

/// Scan and delete all keys at a given prefix
pub async fn scan_and_delete(conn: &mut Connection, prefix: &str) -> RedisResult<()> {
    // Initialize the cursor for SCAN
    let mut cursor: u64 = 0;

    loop {
        // Use SCAN to get a batch of keys with the specified prefix
        let (next_cursor, keys): (u64, Vec<String>) = redis::cmd("SCAN")
            .cursor_arg(cursor)
            .arg("MATCH")
            .arg(format!("{prefix}*"))
            .arg("COUNT")
            .arg(100)
            .query_async(conn)
            .await?;
        cursor = next_cursor;

        // Delete keys if any are found
        if !keys.is_empty() {
            for key in keys {
                // NOTE: <_, ()> is required to avoid the dependency_on_unit_never_type_fallback,
                // which will be an error in the future. It may look like black magic, but you can
                // simple think of it as the compiler winking at you.
                // See Rust issue #123748 <https://github.com/rust-lang/rust/issues/123748>
                conn.del::<_, ()>(key).await?;
            }
        }

        // Exit the loop if we've scanned all keys
        if cursor == 0 {
            break;
        }
    }

    Ok(())
}
