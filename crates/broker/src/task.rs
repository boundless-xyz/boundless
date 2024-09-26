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

/// Start and monitor a RetryTask impl with [count] tasks
pub async fn supervisor(count: usize, task: Arc<impl RetryTask>) -> AnyhowRes<()> {
    let mut tasks = JoinSet::new();

    for i in 0..count {
        tracing::debug!("Spawning task: {i}");
        tasks.spawn(task.spawn());
    }

    while let Some(res) = tasks.join_next().await {
        match res {
            Ok(task_res) => match task_res {
                Ok(_) => tracing::debug!("Task exited cleanly"),
                Err(err) => match err {
                    SupervisorErr::Recover(err) => {
                        tracing::error!(
                            "Recoverable failure detected: {err:?}, spawning replacement"
                        );
                        tasks.spawn(task.spawn());
                    }
                    SupervisorErr::Fault(err) => {
                        tracing::error!("FAULT: Hard failure detect: {err:?}");
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
        let count = 2;
        task.tx(0).await.unwrap();
        let supervisor_task = supervisor(count, task.clone());

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
        let count = 2;
        task.tx(0).await.unwrap();

        let supervisor_task = supervisor(count, task.clone());
        task.tx(3).await.unwrap();
        task.close();

        supervisor_task.await.unwrap();
    }
}
