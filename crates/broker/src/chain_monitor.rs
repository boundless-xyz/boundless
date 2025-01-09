use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc,
};

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
    current_block_number: Arc<AtomicU64>,
}

impl<P> ChainMonitorService<P>
where
    P: Provider<BoxTransport, Ethereum> + 'static + Clone,
{
    pub async fn new(provider: Arc<P>, config: ConfigLock) -> Result<Self> {
        let current_block_number = Arc::new(AtomicU64::new(provider.get_block_number().await?));

        Ok(Self { provider, config, current_block_number })
    }

    /// Returns the latest block number.
    pub fn current_block_number(&self) -> u64 {
        self.current_block_number.load(Ordering::Acquire)
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
            loop {
                let conf_poll_time_ms = {
                    let config = self_clone
                        .config
                        .lock_all()
                        .context("Failed to lock config")
                        .map_err(SupervisorErr::Fault)?;
                    config.prover.status_poll_ms
                };

                let block_number = self_clone
                    .provider
                    .get_block_number()
                    .await
                    .context("Failed to get block number")
                    .map_err(SupervisorErr::Recover)?;
                self_clone.current_block_number.store(block_number, Ordering::Release);
                tokio::time::sleep(tokio::time::Duration::from_millis(conf_poll_time_ms)).await;
            }
        })
    }
}
