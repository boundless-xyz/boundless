// Copyright 2026 Boundless Foundation, Inc.
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

use std::{future::Future, pin::Pin, sync::Arc, time::Duration};

use crate::{config::ConfigLock, errors::CodedError};
use anyhow::{Context, Result as AnyhowRes};
use thiserror::Error;
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;
use tracing::{Instrument, Span};

#[derive(Error, Debug)]
pub enum SupervisorErr<E: CodedError> {
    /// Restart / replace the task after failure
    #[error("{code} Recoverable error: {0}", code = self.code())]
    Recover(E),
    /// Hard failure and exit the task set
    #[error("{code} Hard failure: {0}", code = self.code())]
    Fault(E),
}

const FAULT_CODE: &str = "[B-SUP-FAULT]";

impl<E: CodedError> CodedError for SupervisorErr<E> {
    fn code(&self) -> &str {
        match self {
            SupervisorErr::Recover(_) => "[B-SUP-RECOVER]",
            SupervisorErr::Fault(_) => FAULT_CODE,
        }
    }
}

#[allow(type_alias_bounds)]
pub type RetryRes<E: CodedError> =
    Pin<Box<dyn Future<Output = Result<(), SupervisorErr<E>>> + Send + 'static>>;

pub trait RetryTask {
    type Error: CodedError;
    /// Defines how to spawn a task to be monitored for restarts
    fn spawn(&self, cancel_token: CancellationToken) -> RetryRes<Self::Error>;
}

/// Simplified service trait for broker components.
///
/// Implement this instead of [`RetryTask`] directly. The blanket impl below
/// turns any `BrokerService` into a `RetryTask` automatically, so service
/// authors only have to write the actual loop body — no `Box::pin`, no
/// `self.clone()` ceremony at the trait boundary.
///
/// # Ownership
///
/// `run` takes `self` by value. The blanket impl clones the service once per
/// supervisor restart, so implementations are free to consume `self` in the
/// loop without juggling clones internally.
///
/// # Send / 'static
///
/// The returned future must be `Send + 'static` so it can be polled across
/// threads in tokio's multi-thread runtime and outlive the supervisor frame.
pub trait BrokerService: Clone + Send + Sync + 'static {
    type Error: CodedError + Send + Sync + 'static;

    fn run(
        self,
        cancel_token: CancellationToken,
    ) -> impl Future<Output = Result<(), SupervisorErr<Self::Error>>> + Send + 'static;
}

impl<T: BrokerService> RetryTask for T {
    type Error = T::Error;

    fn spawn(&self, cancel_token: CancellationToken) -> RetryRes<Self::Error> {
        let this = self.clone();
        Box::pin(this.run(cancel_token))
    }
}

/// Configuration for retry behavior in the supervisor
#[derive(Debug, Clone)]
pub(crate) struct RetryPolicy {
    /// Initial delay between retry attempts
    pub delay: Duration,
    /// Multiplier applied to the delay after each retry
    pub backoff_multiplier: f64,
    /// Maximum delay between retries, regardless of backoff
    pub max_delay: Duration,
    /// Duration after which to reset the retry counter if a task runs successfully
    pub reset_after: Option<Duration>,
    pub(crate) critical: bool,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            delay: std::time::Duration::from_millis(500),
            backoff_multiplier: 1.5,
            max_delay: std::time::Duration::from_secs(60),
            // Reset the backoff after 5 minutes of running without a failure.
            reset_after: Some(std::time::Duration::from_secs(60 * 5)),
            critical: false,
        }
    }
}

impl RetryPolicy {
    pub(crate) const CRITICAL_SERVICE: RetryPolicy = RetryPolicy {
        delay: std::time::Duration::from_millis(100),
        backoff_multiplier: 1.5,
        max_delay: std::time::Duration::from_secs(2),
        reset_after: Some(std::time::Duration::from_secs(60)),
        critical: true,
    };
}

/// Supervisor for managing and monitoring tasks with retry capabilities
pub(crate) struct Supervisor<T: RetryTask> {
    /// The task to be supervised
    task: Arc<T>,
    /// Configuration for retry behavior
    retry_policy: RetryPolicy,
    config: ConfigLock,
    /// Cancellation token for graceful shutdown
    cancel_token: CancellationToken,
}

impl<T: RetryTask> Supervisor<T>
where
    T: Send,
    T::Error: Send + Sync + 'static,
{
    /// Create a new supervisor with a single task
    pub fn new(task: Arc<T>, config: ConfigLock, cancel_token: CancellationToken) -> Self {
        Self { task, retry_policy: RetryPolicy::default(), config, cancel_token }
    }

    /// Configure the retry policy
    pub fn with_retry_policy(mut self, retry_policy: RetryPolicy) -> Self {
        self.retry_policy = retry_policy;
        self
    }

    /// Run the supervisor, monitoring tasks and handling retries
    pub async fn spawn(self) -> AnyhowRes<()> {
        let mut tasks = JoinSet::new();
        let mut retry_count = 0;
        let mut current_delay = self.retry_policy.delay;
        let mut last_spawn_time = std::time::Instant::now();
        let parent_span = Span::current();

        // Spawn initial task
        tracing::debug!("Spawning task");
        tasks.spawn(self.task.spawn(self.cancel_token.clone()).instrument(parent_span.clone()));

        while let Some(res) = tasks.join_next().await {
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
                    current_delay = self.retry_policy.delay; // Reset delay to initial value
                }
            }
            match res {
                Ok(task_res) => match task_res {
                    Ok(()) => {
                        tracing::debug!("Task exited cleanly");
                    }
                    Err(ref supervisor_err) => match supervisor_err {
                        SupervisorErr::Recover(ref _err) => {
                            if self.retry_policy.critical {
                                let max_retries = {
                                    let config =
                                        self.config.lock_all().context("Failed to read config")?;
                                    config.prover.max_critical_task_retries
                                };

                                // Check if we've exceeded max retries
                                if retry_count >= max_retries {
                                    // We manually log the fault code rather than rendering the SupervisorErr::Recover
                                    // code so that we indicate we are now in a hard fault state after exhausting retries.
                                    tracing::error!(
                                        "{} Exceeded maximum retries ({max_retries}) for task",
                                        FAULT_CODE
                                    );
                                    anyhow::bail!("Exceeded maximum retries for task");
                                }
                            }

                            tracing::warn!(
                                "{}, spawning replacement (retry {})",
                                supervisor_err,
                                retry_count + 1,
                            );
                            tracing::debug!("Waiting {:?} before retry", current_delay);

                            // Instead of sleeping here, wrap the task spawn with a delay
                            let task_clone = self.task.clone();
                            let t = task_clone.spawn(self.cancel_token.clone());
                            tasks.spawn(
                                async move {
                                    // Apply calculated retry delay before spawning the task
                                    tokio::time::sleep(current_delay).await;
                                    t.await
                                }
                                .instrument(parent_span.clone()),
                            );

                            retry_count += 1;
                            last_spawn_time = std::time::Instant::now() + current_delay;

                            // Update the delay for next retry, ensuring it doesn't exceed max_delay
                            current_delay = current_delay
                                .mul_f64(self.retry_policy.backoff_multiplier)
                                .min(self.retry_policy.max_delay);
                        }
                        SupervisorErr::Fault(err) => {
                            tracing::error!("{}", supervisor_err);
                            anyhow::bail!("Hard failure in supervisor task: {err}");
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
    use thiserror::Error;
    use tracing_test::traced_test;

    struct TestTask {
        tx: Sender<u32>,
        rx: Receiver<u32>,
    }

    #[derive(Error, Debug)]
    enum TestErr {
        #[error("Sample error: {0}")]
        SampleErr(anyhow::Error),
    }

    impl CodedError for TestErr {
        fn code(&self) -> &str {
            match self {
                TestErr::SampleErr(_) => "[B-TEST-001]",
            }
        }
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

        async fn process_item(
            rx: Receiver<u32>,
            cancel_token: CancellationToken,
        ) -> Result<(), SupervisorErr<TestErr>> {
            loop {
                tokio::select! {
                    // Handle incoming values
                    result = rx.recv() => {
                        let value = match result {
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
                            2 => {
                                return Err(SupervisorErr::Recover(TestErr::SampleErr(anyhow::anyhow!(
                                    "Sample error"
                                ))))
                            }
                            // Mock a hard failure
                            3 => {
                                return Err(SupervisorErr::Fault(TestErr::SampleErr(anyhow::anyhow!(
                                    "FAILURE"
                                ))))
                            }
                            _ => {
                                return Err(SupervisorErr::Recover(TestErr::SampleErr(anyhow::anyhow!(
                                    "UNKNOWN VALUE TYPE"
                                ))))
                            }
                        }
                    }
                    // Handle cancellation
                    _ = cancel_token.cancelled() => {
                        tracing::debug!("Task cancelled, exiting cleanly");
                        break;
                    }
                }
            }

            Ok(())
        }
    }

    impl RetryTask for TestTask {
        type Error = TestErr;
        fn spawn(&self, cancel_token: CancellationToken) -> RetryRes<Self::Error> {
            let rx_copy = self.rx.clone();
            Box::pin(Self::process_item(rx_copy, cancel_token))
        }
    }

    #[tokio::test]
    #[traced_test]
    async fn supervisor_simple() {
        let task = Arc::new(TestTask::new());
        task.tx(0).await.unwrap();

        let supervisor_task =
            Supervisor::new(task.clone(), ConfigLock::default(), CancellationToken::new()).spawn();

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

        let supervisor_task =
            Supervisor::new(task.clone(), ConfigLock::default(), CancellationToken::new()).spawn();

        task.tx(3).await.unwrap();
        task.close();

        supervisor_task.await.unwrap();
    }

    #[tokio::test]
    #[traced_test]
    async fn supervisor_with_retry_policy() {
        let task = Arc::new(TestTask::new());
        let config = ConfigLock::default();
        config.load_write().unwrap().prover.max_critical_task_retries = 3;

        let supervisor_task = Supervisor::new(task.clone(), config, CancellationToken::new())
            .with_retry_policy(RetryPolicy {
                delay: std::time::Duration::from_millis(10),
                backoff_multiplier: 2.0,
                max_delay: std::time::Duration::from_millis(500),
                reset_after: None,
                critical: true,
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

    #[derive(Clone)]
    struct BrokerServiceTask {
        // Channel the test uses to drive the service. Wrapped in Arc so the
        // service can be Clone (the supervisor clones on retry).
        rx: Arc<tokio::sync::Mutex<tokio::sync::mpsc::Receiver<u32>>>,
    }

    impl BrokerService for BrokerServiceTask {
        type Error = TestErr;

        async fn run(
            self,
            cancel_token: CancellationToken,
        ) -> Result<(), SupervisorErr<Self::Error>> {
            let mut rx = self.rx.lock().await;
            loop {
                tokio::select! {
                    msg = rx.recv() => {
                        match msg {
                            Some(0) => continue,
                            Some(2) => return Err(SupervisorErr::Recover(TestErr::SampleErr(
                                anyhow::anyhow!("recoverable"),
                            ))),
                            Some(_) | None => break,
                        }
                    }
                    _ = cancel_token.cancelled() => break,
                }
            }
            Ok(())
        }
    }

    #[tokio::test]
    #[traced_test]
    async fn broker_service_blanket_impl_works_with_supervisor() {
        // A type implementing BrokerService gets RetryTask via the blanket impl,
        // so it can be supervised without writing any Box::pin / self.clone() code.
        let (tx, rx) = tokio::sync::mpsc::channel(16);
        let task = Arc::new(BrokerServiceTask { rx: Arc::new(tokio::sync::Mutex::new(rx)) });

        let supervisor_task =
            Supervisor::new(task, ConfigLock::default(), CancellationToken::new()).spawn();

        tx.send(0).await.unwrap();
        // Trigger one recoverable error -> supervisor should restart the service.
        tx.send(2).await.unwrap();
        tx.send(0).await.unwrap();
        // Closing the channel ends the loop cleanly.
        drop(tx);

        supervisor_task.await.unwrap();
    }

    #[tokio::test]
    #[traced_test]
    async fn supervisor_cancellation() {
        let task = Arc::new(TestTask::new());
        let cancel_token = CancellationToken::new();

        let supervisor_task =
            Supervisor::new(task.clone(), ConfigLock::default(), cancel_token.clone()).spawn();

        task.tx(0).await.unwrap();
        task.tx(0).await.unwrap();

        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

        cancel_token.cancel();

        supervisor_task.await.unwrap();
        assert!(logs_contain("Task cancelled, exiting cleanly"));
    }
}
