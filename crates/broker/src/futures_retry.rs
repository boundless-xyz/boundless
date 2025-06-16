// Copyright 2025 RISC Zero, Inc.
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

use std::future::Future;
use tokio::time::Duration;

/// Retry a future with a specified number of retries and sleep duration between attempts.
pub async fn retry<T, E, F, Fut>(
    retry_count: u64,
    retry_sleep_ms: u64,
    operation: F,
    function_name: &str,
) -> Result<T, E>
where
    F: Fn() -> Fut,
    Fut: Future<Output = Result<T, E>>,
    E: std::fmt::Debug,
{
    if retry_count == 0 {
        return operation().await;
    }

    let mut last_error = None;
    for attempt in 0..=retry_count {
        match operation().await {
            Ok(result) => return Ok(result),
            Err(err) => {
                if attempt < retry_count {
                    tracing::warn!(
                        "Operation [{}] failed: {err:?}, starting retry {}/{}",
                        function_name,
                        attempt + 1,
                        retry_count
                    );
                    tokio::time::sleep(Duration::from_millis(retry_sleep_ms)).await;
                    last_error = Some(err);
                    continue;
                }
                last_error = Some(err);
            }
        }
    }

    tracing::warn!(
        "Operation [{}] failed after {} retries, returning last error: {:?}",
        function_name,
        retry_count,
        last_error
    );
    Err(last_error.unwrap())
}

/// Retry a future with a specified number of retries and sleep duration between attempts.
/// Only retries if the error matches the predicate function.
pub async fn retry_only<T, E, F, Fut>(
    retry_count: u64,
    retry_sleep_ms: u64,
    operation: F,
    function_name: &str,
    should_retry: impl Fn(&E) -> bool,
) -> Result<T, E>
where
    F: Fn() -> Fut,
    Fut: Future<Output = Result<T, E>>,
    E: std::fmt::Debug,
{
    if retry_count == 0 {
        return operation().await;
    }

    let mut last_error = None;
    for attempt in 0..=retry_count {
        match operation().await {
            Ok(result) => return Ok(result),
            Err(err) => {
                if !should_retry(&err) {
                    tracing::warn!(
                        "Operation [{}] failed with non-retryable error: {err:?}, not retrying",
                        function_name
                    );
                    return Err(err);
                }

                if attempt < retry_count {
                    tracing::warn!(
                        "Operation [{}] failed: {err:?}, starting retry {}/{}",
                        function_name,
                        attempt + 1,
                        retry_count
                    );
                    tokio::time::sleep(Duration::from_millis(retry_sleep_ms)).await;
                    last_error = Some(err);
                    continue;
                }
                last_error = Some(err);
            }
        }
    }

    tracing::warn!(
        "Operation [{}] failed after {} retries, returning last error: {:?}",
        function_name,
        retry_count,
        last_error
    );
    Err(last_error.unwrap())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        atomic::{AtomicU32, Ordering},
        Arc,
    };
    use thiserror::Error;
    use tracing_test::traced_test;

    #[derive(Error, Debug, PartialEq)]
    enum TestError {
        #[error("Retryable")]
        Retryable,
        #[error("NonRetryable")]
        NonRetryable,
    }

    #[tokio::test]
    #[traced_test]
    async fn test_retry_success() {
        let counter = Arc::new(AtomicU32::new(0));
        let counter_clone = counter.clone();

        let result = retry(
            2,
            0,
            || {
                let counter = counter_clone.clone();
                async move {
                    let current = counter.fetch_add(1, Ordering::SeqCst);
                    if current == 0 || current == 1 {
                        Err("Attempt failed")
                    } else {
                        Ok(current)
                    }
                }
            },
            "test operation",
        )
        .await;

        assert_eq!(result.unwrap(), 2);
        assert_eq!(counter.load(Ordering::SeqCst), 3);
        assert!(logs_contain(
            "Operation [test operation] failed: \"Attempt failed\", starting retry 1/2"
        ));
    }

    #[tokio::test]
    #[traced_test]
    async fn test_retry_single_attempt() {
        let counter = Arc::new(AtomicU32::new(0));
        let counter_clone = counter.clone();

        let result = retry(
            1,
            0,
            || {
                let counter = counter_clone.clone();
                async move {
                    let current = counter.fetch_add(1, Ordering::SeqCst);
                    if current == 0 {
                        Err("First attempt failed")
                    } else {
                        Ok(current)
                    }
                }
            },
            "test operation",
        )
        .await;

        assert_eq!(result.unwrap(), 1);
        assert_eq!(counter.load(Ordering::SeqCst), 2);
        assert!(logs_contain(
            "Operation [test operation] failed: \"First attempt failed\", starting retry 1/1"
        ));
    }

    #[tokio::test]
    #[traced_test]
    async fn test_retry_exhausted() {
        let counter = Arc::new(AtomicU32::new(0));
        let counter_clone = counter.clone();

        let result: Result<(), &str> = retry(
            2,
            0,
            || {
                let counter = counter_clone.clone();
                async move {
                    counter.fetch_add(1, Ordering::SeqCst);
                    Err("Always fails")
                }
            },
            "test operation",
        )
        .await;

        assert!(result.is_err());
        assert_eq!(counter.load(Ordering::SeqCst), 3);
        assert!(logs_contain(
            "Operation [test operation] failed: \"Always fails\", starting retry 1/2"
        ));
    }

    #[tokio::test]
    #[traced_test]
    async fn test_retry_only_specific_errors() {
        let counter = Arc::new(AtomicU32::new(0));
        let counter_clone = counter.clone();

        let result = retry_only(
            5,
            0,
            || {
                let counter = counter_clone.clone();
                async move {
                    let current = counter.fetch_add(1, Ordering::SeqCst);
                    match current {
                        0 => Err(TestError::Retryable),
                        1 => Err(TestError::NonRetryable),
                        _ => Ok(current),
                    }
                }
            },
            "test operation",
            |err| matches!(err, TestError::Retryable),
        )
        .await;

        assert!(result.is_err());
        assert_eq!(counter.load(Ordering::SeqCst), 2);
        assert!(logs_contain("Operation [test operation] failed: Retryable, starting retry 1/5"));
        assert!(logs_contain("Operation [test operation] failed with non-retryable error: NonRetryable, not retrying"));
    }
}
