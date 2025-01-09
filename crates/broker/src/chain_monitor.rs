use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;
use tokio::sync::watch;
use tokio::sync::Notify;
use tokio::sync::RwLock;

use alloy::{network::Ethereum, providers::Provider, transports::BoxTransport};
use anyhow::{Context, Result};

use crate::{
    config::ConfigLock,
    task::{RetryRes, RetryTask, SupervisorErr},
};

#[derive(Clone)]
pub struct ChainMonitorService<P> {
    provider: Arc<P>,
    config: ConfigLock,
    block_number: watch::Sender<u64>,
    update_notifier: Arc<Notify>,
    next_update: Arc<RwLock<Instant>>,
}

impl<P> ChainMonitorService<P>
where
    P: Provider<BoxTransport, Ethereum> + 'static + Clone,
{
    pub async fn new(provider: Arc<P>, config: ConfigLock) -> Result<Self> {
        let (block_number, _) = watch::channel(0);

        Ok(Self {
            provider,
            config,
            block_number,
            update_notifier: Arc::new(Notify::new()),
            next_update: Arc::new(RwLock::new(Instant::now())),
        })
    }

    /// Returns the latest block number, triggering an update if enough time has passed
    pub async fn current_block_number(&self) -> Result<u64> {
        if Instant::now() > *self.next_update.read().await {
            let mut rx = self.block_number.subscribe();
            self.update_notifier.notify_one();
            rx.changed().await.context("failed to query block number from chain monitor")?;
            let block_number = *rx.borrow();
            Ok(block_number)
        } else {
            Ok(*self.block_number.borrow())
        }
    }
}

impl<P> RetryTask for ChainMonitorService<P>
where
    P: Provider<BoxTransport, Ethereum> + 'static + Clone,
{
    fn spawn(&self) -> RetryRes {
        let self_clone = self.clone();

        Box::pin(async move {
            tracing::info!("Starting ChainMonitor service");
            let conf_poll_time_ms = {
                let config = self_clone
                    .config
                    .lock_all()
                    .context("Failed to lock config")
                    .map_err(SupervisorErr::Fault)?;
                config.prover.status_poll_ms
            };

            loop {
                // Wait for notification
                self_clone.update_notifier.notified().await;
                // Needs update, lock next update value to avoid unnecessary notifications.
                let mut next_update = self_clone.next_update.write().await;

                let block_number = self_clone
                    .provider
                    .get_block_number()
                    .await
                    .context("Failed to get block number")
                    .map_err(SupervisorErr::Recover)?;
                let _ = self_clone.block_number.send_replace(block_number);

                // Set timestamp for next update
                *next_update = Instant::now() + Duration::from_millis(conf_poll_time_ms);
            }
        })
    }
}
