// Copyright (c) 2025 RISC Zero, Inc.
//
// All rights reserved.

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
    for attempt in 0..retry_count {
        match operation().await {
            Ok(result) => return Ok(result),
            Err(err) => {
                if attempt < retry_count - 1 {
                    tracing::warn!(
                        "Operation [{}] failed: {err:?}, retrying {}/{}",
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
        "Operation [{}] failed after {} attempts, returning {:?}",
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
    use tracing_test::traced_test;

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
            "Operation [test operation] failed: \"First attempt failed\", retrying 1/2"
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
        assert_eq!(counter.load(Ordering::SeqCst), 2);
        assert!(logs_contain("Operation [test operation] failed: \"Always fails\", retrying 1/2"));
    }
}
