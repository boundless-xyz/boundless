// Copyright (c) 2024 RISC Zero, Inc.
//
// All rights reserved.

use std::{future::Future, pin::Pin, sync::Arc};

use anyhow::{Error as AnyhowErr, Result as AnyhowRes};
use thiserror::Error;
use tokio::task::JoinSet;

#[derive(Error, Debug)]
pub enum SupervisorErr {
    /// Restart / replace the task after failure
    #[error("Recoverable error: {0}")]
    Recover(AnyhowErr),
    /// Hard failure and exit the task set
    #[error("Hard failure: {0}")]
    Fault(AnyhowErr),
}

pub type RetryRes = Pin<Box<dyn Future<Output = Result<(), SupervisorErr>> + Send + 'static>>;

pub trait RetryTask {
    /// Defines how to spawn a task to be monitored for restarts
    fn spawn(&self) -> RetryRes;
}

/// Configuration for retry behavior in the supervisor
#[derive(Debug, Clone)]
pub(crate) struct RetryPolicy {
    /// Initial delay between retry attempts
    pub delay: std::time::Duration,
    /// Multiplier applied to the delay after each retry
    pub backoff_multiplier: f64,
    /// Maximum delay between retries, regardless of backoff
    pub max_delay: std::time::Duration,
    /// Maximum number of consecutive retries before giving up (None for unlimited)
    pub max_retries: Option<usize>,
    /// Duration after which to reset the retry counter if a task runs successfully
    pub reset_after: Option<std::time::Duration>,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            delay: std::time::Duration::from_millis(500),
            backoff_multiplier: 1.5,
            max_delay: std::time::Duration::from_secs(60),
            max_retries: None,
            // Reset the backoff after 5 minutes of running without a failure.
            reset_after: Some(std::time::Duration::from_secs(60 * 5)),
        }
    }
}

impl RetryPolicy {
    pub(crate) const CRITICAL_SERVICE: RetryPolicy = RetryPolicy {
        delay: std::time::Duration::from_millis(100),
        backoff_multiplier: 1.5,
        max_delay: std::time::Duration::from_secs(2),
        max_retries: None,
        reset_after: Some(std::time::Duration::from_secs(60)),
    };
}

/// Supervisor for managing and monitoring tasks with retry capabilities
pub(crate) struct Supervisor<T: RetryTask> {
    /// The task to be supervised
    task: Arc<T>,
    /// Configuration for retry behavior
    retry_policy: RetryPolicy,
}

impl<T> Supervisor<T>
where
    T: RetryTask + Send,
{
    /// Create a new supervisor with a single task
    pub fn new(task: Arc<T>) -> Self {
        Self { task, retry_policy: RetryPolicy::default() }
    }

    /// Configure the retry policy
    pub fn with_retry_policy(mut self, retry_policy: RetryPolicy) -> Self {
        self.retry_policy = retry_policy;
        self
    }

    /// Calculate the delay for a specific retry attempt
    fn calculate_retry_delay(&self, retry_count: usize) -> std::time::Duration {
        if retry_count == 0 {
            return self.retry_policy.delay;
        }

        let backoff = self.retry_policy.delay.as_millis() as f64
            * self.retry_policy.backoff_multiplier.powi(retry_count as i32);

        let backoff_ms = backoff.min(self.retry_policy.max_delay.as_millis() as f64) as u64;

        std::time::Duration::from_millis(backoff_ms)
    }

    /// Run the supervisor, monitoring tasks and handling retries
    pub async fn spawn(self) -> AnyhowRes<()> {
        let mut tasks = JoinSet::new();
        let mut retry_count = 0;
        let mut last_spawn_time = std::time::Instant::now();

        // Spawn initial task
        tracing::debug!("Spawning task");
        tasks.spawn(self.task.spawn());

        while let Some(res) = tasks.join_next().await {
            match res {
                Ok(task_res) => match task_res {
                    Ok(_) => {
                        tracing::debug!("Task exited cleanly");
                        // Check if we should reset the retry counter based on how long the task ran
                        if let Some(reset_duration) = self.retry_policy.reset_after {
                            let task_duration = last_spawn_time.elapsed();
                            if task_duration >= reset_duration && retry_count > 0 {
                                tracing::info!(
                                    "Task ran successfully for {:?}, resetting retry counter from {}",
                                    task_duration,
                                    retry_count
                                );
                                retry_count = 0;
                            }
                        }
                    }
                    Err(err) => match err {
                        SupervisorErr::Recover(err) => {
                            // Check if we've exceeded max retries
                            if let Some(max) = self.retry_policy.max_retries {
                                if retry_count >= max {
                                    tracing::error!("Exceeded maximum retries ({max}) for task");
                                    anyhow::bail!("Exceeded maximum retries for task");
                                }
                            }

                            let delay = self.calculate_retry_delay(retry_count);

                            tracing::warn!(
                                "Recoverable failure detected: {err:?}, spawning replacement (retry {}/{})",
                                retry_count + 1,
                                self.retry_policy.max_retries.map_or("∞".to_string(), |m| m.to_string())
                            );
                            tracing::debug!("Waiting {:?} before retry", delay);

                            // Instead of sleeping here, wrap the task spawn with a delay
                            let task_clone = self.task.clone();
                            let t = task_clone.spawn();
                            tasks.spawn(async move {
                                // Apply calculated retry delay before spawning the task
                                tokio::time::sleep(delay).await;
                                t.await
                            });

                            retry_count += 1;
                            last_spawn_time = std::time::Instant::now() + delay;
                        }
                        SupervisorErr::Fault(err) => {
                            tracing::error!("FAULT: Hard failure detected: {err:?}");
                            anyhow::bail!("Hard failure in supervisor task");
                        }
                    },
                },
                Err(err) => {
                    if err.is_cancelled() {
                        tracing::warn!("Task was canceled, treating it like a clean exit");
                    } else {
                        tracing::error!("ABORT: supervisor join failed");
                        anyhow::bail!(err);
                    }
                }
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::Context;
    use async_channel::{Receiver, Sender};
    use tracing_test::traced_test;

    struct TestTask {
        tx: Sender<u32>,
        rx: Receiver<u32>,
    }

    impl TestTask {
        fn new() -> Self {
            let (tx, rx) = async_channel::bounded(100);
            Self { tx, rx }
        }

        async fn tx(&self, val: u32) -> AnyhowRes<()> {
            self.tx.send(val).await.context("Failed to send on tx")
        }

        fn close(&self) -> bool {
            self.tx.close()
        }

        async fn process_item(rx: Receiver<u32>) -> Result<(), SupervisorErr> {
            loop {
                let value = match rx.recv().await {
                    Ok(val) => val,
                    Err(_) => {
                        tracing::debug!("channel closed, exiting..");
                        break;
                    }
                };

                tracing::info!("Got value: {value}");

                match value {
                    // Mock do work
                    0 => tokio::time::sleep(tokio::time::Duration::from_millis(100)).await,
                    // mock a clean exit
                    1 => return Ok(()),
                    // Mock a soft failure
                    2 => return Err(SupervisorErr::Recover(anyhow::anyhow!("Sample error"))),
                    // Mock a hard failure
                    3 => return Err(SupervisorErr::Fault(anyhow::anyhow!("FAILURE"))),
                    _ => return Err(SupervisorErr::Recover(anyhow::anyhow!("UNKNOWN VALUE TYPE"))),
                }
            }

            Ok(())
        }
    }

    impl RetryTask for TestTask {
        fn spawn(&self) -> RetryRes {
            let rx_copy = self.rx.clone();
            Box::pin(Self::process_item(rx_copy))
        }
    }

    #[tokio::test]
    #[traced_test]
    async fn supervisor_simple() {
        let task = Arc::new(TestTask::new());
        task.tx(0).await.unwrap();

        let supervisor_task = Supervisor::new(task.clone()).spawn();

        task.tx(0).await.unwrap();
        task.tx(0).await.unwrap();
        task.tx(2).await.unwrap();
        task.tx(0).await.unwrap();
        task.close();

        supervisor_task.await.unwrap();
    }

    #[tokio::test]
    #[traced_test]
    #[should_panic(expected = "Hard failure in supervisor task")]
    async fn supervisor_fault() {
        let task = Arc::new(TestTask::new());
        task.tx(0).await.unwrap();

        let supervisor_task = Supervisor::new(task.clone()).spawn();

        task.tx(3).await.unwrap();
        task.close();

        supervisor_task.await.unwrap();
    }

    #[tokio::test]
    #[traced_test]
    async fn supervisor_with_retry_policy() {
        let task = Arc::new(TestTask::new());

        let supervisor_task = Supervisor::new(task.clone())
            .with_retry_policy(RetryPolicy {
                delay: std::time::Duration::from_millis(10),
                backoff_multiplier: 2.0,
                max_delay: std::time::Duration::from_millis(500),
                max_retries: Some(3),
                reset_after: None,
            })
            .spawn();

        // Trigger 3 recoverable errors
        task.tx(2).await.unwrap();
        task.tx(2).await.unwrap();
        task.tx(2).await.unwrap();
        // Then a successful task
        task.tx(0).await.unwrap();

        task.tx(2).await.unwrap();
        task.close();


        let res = supervisor_task.await;
        assert!(res.unwrap_err().to_string().contains("Exceeded maximum retries for task"));
    }
}
