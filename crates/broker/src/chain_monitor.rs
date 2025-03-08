// Copyright (c) 2025 RISC Zero, Inc.
//
// All rights reserved.

use alloy_chains::NamedChain;
use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;
use tokio::sync::watch;
use tokio::sync::Notify;
use tokio::sync::RwLock;

use alloy::{
    network::Ethereum, providers::Provider, rpc::types::BlockTransactionsKind,
    transports::BoxTransport,
};
use anyhow::{Context, Result};

use crate::task::{RetryRes, RetryTask, SupervisorErr};

#[derive(Clone)]
pub struct ChainMonitorService<P> {
    provider: Arc<P>,
    block_number: watch::Sender<u64>,
    block_timestamp: Arc<RwLock<Option<u64>>>,
    update_notifier: Arc<Notify>,
    next_update: Arc<RwLock<Instant>>,
}

impl<P> ChainMonitorService<P>
where
    P: Provider<BoxTransport, Ethereum> + 'static + Clone,
{
    pub async fn new(provider: Arc<P>) -> Result<Self> {
        let (block_number, _) = watch::channel(0);

        Ok(Self {
            provider,
            block_number,
            block_timestamp: Arc::new(RwLock::new(None)),
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
            // Clear the block timestamp cache.
            self.block_timestamp.write().await.take();
            let block_number = *rx.borrow();
            Ok(block_number)
        } else {
            Ok(*self.block_number.borrow())
        }
    }

    /// Returns the latest block timestamp, triggering an update if enough time has passed
    pub async fn current_block_timestamp(&self) -> Result<u64> {
        // Get the current_block_number. This may clear the timestamp cache.
        let block_number = self.current_block_number().await?;
        let cached_timestamp: Option<u64> = *self.block_timestamp.read().await;
        if let Some(ts) = cached_timestamp {
            return Ok(ts);
        }
        let current_timestamp = self
            .provider
            .get_block_by_number(block_number.into(), BlockTransactionsKind::Hashes)
            .await
            .with_context(|| format!("failed to get block {block_number}"))?
            .with_context(|| format!("failed to get block {block_number}: block not found"))?
            .header
            .timestamp;
        *self.block_timestamp.write().await = Some(current_timestamp);
        Ok(current_timestamp)
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

            let chain_id = self_clone
                .provider
                .get_chain_id()
                .await
                .context("failed to get chain ID")
                .map_err(SupervisorErr::Recover)?;

            let chain_poll_time = NamedChain::try_from(chain_id)
                .ok()
                .and_then(|chain| chain.average_blocktime_hint())
                .map(|block_time| block_time.mul_f32(0.6))
                .unwrap_or(Duration::from_secs(2));

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
                *next_update = Instant::now() + chain_poll_time;
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use alloy::{
        network::EthereumWallet,
        node_bindings::Anvil,
        primitives::U256,
        providers::{ext::AnvilApi, ProviderBuilder},
        signers::local::PrivateKeySigner,
    };

    use super::*;

    #[tokio::test]
    async fn chain_monitor_smoke_test() {
        // Using an unknown chain ID to use default 2s polling time.
        let anvil = Anvil::new().chain_id(888833888).spawn();
        let signer: PrivateKeySigner = anvil.keys()[0].clone().into();
        let provider = Arc::new(
            ProviderBuilder::new()
                .with_recommended_fillers()
                .wallet(EthereumWallet::from(signer))
                .on_builtin(&anvil.endpoint())
                .await
                .unwrap(),
        );

        let chain_monitor = Arc::new(ChainMonitorService::new(provider.clone()).await.unwrap());
        tokio::spawn(chain_monitor.spawn());

        let block = chain_monitor.current_block_number().await.unwrap();
        assert_eq!(block, 0);

        const NUM_BLOCKS: u64 = 10;

        provider.anvil_mine(Some(U256::from(NUM_BLOCKS)), Some(U256::from(2))).await.unwrap();

        // Block should still be 0 until the next polling interval.
        let block = chain_monitor.current_block_number().await.unwrap();
        assert_eq!(block, 0);

        // Update next update time to now, to allow querying the block number from chain.
        *chain_monitor.next_update.write().await = Instant::now();

        let block = chain_monitor.current_block_number().await.unwrap();
        assert_eq!(block, NUM_BLOCKS);
    }
}
