// Copyright (c) 2025 RISC Zero, Inc.
//
// All rights reserved.

use alloy_chains::NamedChain;
use std::{
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::{watch, Notify, RwLock};

use alloy::{eips::BlockNumberOrTag, providers::Provider};
use anyhow::{Context, Result};
use thiserror::Error;

use crate::{
    errors::CodedError,
    task::{RetryRes, RetryTask, SupervisorErr},
};

#[derive(Error, Debug)]
pub enum ChainMonitorErr {
    #[error("{code} Unexpected error: {0}", code = self.code())]
    UnexpectedErr(#[from] anyhow::Error),
}

impl CodedError for ChainMonitorErr {
    fn code(&self) -> &str {
        match self {
            ChainMonitorErr::UnexpectedErr(_) => "[B-CHM-500]",
        }
    }
}

#[derive(Clone)]
pub struct ChainMonitorService<P> {
    provider: Arc<P>,
    block_number: watch::Sender<u64>,
    block_timestamp: watch::Sender<u64>,
    gas_price: watch::Sender<u128>,
    update_notifier: Arc<Notify>,
    next_update: Arc<RwLock<Instant>>,
}

impl<P: Provider> ChainMonitorService<P> {
    pub async fn new(provider: Arc<P>) -> Result<Self> {
        let (block_number, _) = watch::channel(0);
        let (gas_price, _) = watch::channel(0);
        let (block_timestamp, _) = watch::channel(0);

        Ok(Self {
            provider,
            block_number,
            block_timestamp,
            gas_price,
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

    /// Returns the latest block timestamp, triggering an update if enough time has passed
    pub async fn current_block_timestamp(&self) -> Result<u64> {
        if Instant::now() > *self.next_update.read().await {
            let mut rx = self.block_timestamp.subscribe();
            self.update_notifier.notify_one();
            rx.changed().await.context("failed to query block timestamp from chain monitor")?;

            let block_timestamp = *rx.borrow();
            Ok(block_timestamp)
        } else {
            Ok(*self.block_timestamp.borrow())
        }
    }

    /// Returns the gas price (as reported by `eth_gasPrice`) at the latest block.
    /// This triggers an update if enough time has passed.
    pub async fn current_gas_price(&self) -> Result<u128> {
        if Instant::now() > *self.next_update.read().await {
            let mut rx = self.gas_price.subscribe();
            self.update_notifier.notify_one();
            rx.changed().await.context("failed to query gas price from chain monitor")?;

            let gas_price = *rx.borrow();
            Ok(gas_price)
        } else {
            Ok(*self.gas_price.borrow())
        }
    }
}

impl<P> RetryTask for ChainMonitorService<P>
where
    P: Provider + 'static + Clone,
{
    type Error = ChainMonitorErr;
    fn spawn(&self) -> RetryRes<Self::Error> {
        let self_clone = self.clone();

        Box::pin(async move {
            tracing::info!("Starting ChainMonitor service");

            let chain_id = self_clone
                .provider
                .get_chain_id()
                .await
                .context("failed to get chain ID")
                .map_err(ChainMonitorErr::UnexpectedErr)
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

                // Get the lastest block and gas price.
                let (block_res, gas_price_res) = tokio::join!(
                    self_clone.provider.get_block_by_number(BlockNumberOrTag::Latest),
                    self_clone.provider.get_gas_price()
                );

                let block = block_res
                    .context("failed to latest block")
                    .map_err(ChainMonitorErr::UnexpectedErr)
                    .map_err(SupervisorErr::Recover)?
                    .context("failed to fetch latest block: no block in response")
                    .map_err(ChainMonitorErr::UnexpectedErr)
                    .map_err(SupervisorErr::Recover)?;
                let _ = self_clone.block_number.send_replace(block.header.number);
                let _ = self_clone.block_timestamp.send_replace(block.header.timestamp);

                let gas_price = gas_price_res
                    .context("failed to get gas price")
                    .map_err(ChainMonitorErr::UnexpectedErr)
                    .map_err(SupervisorErr::Recover)?;
                let _ = self_clone.gas_price.send_replace(gas_price);

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
                .wallet(EthereumWallet::from(signer))
                .connect(&anvil.endpoint())
                .await
                .unwrap(),
        );

        let chain_monitor = Arc::new(ChainMonitorService::new(provider.clone()).await.unwrap());
        tokio::spawn(chain_monitor.spawn());

        let block = chain_monitor.current_block_number().await.unwrap();
        assert_eq!(block, 0);

        const NUM_BLOCKS: u64 = 10;

        provider.anvil_mine(Some(NUM_BLOCKS), Some(2)).await.unwrap();

        // Block should still be 0 until the next polling interval.
        let block = chain_monitor.current_block_number().await.unwrap();
        assert_eq!(block, 0);

        // Update next update time to now, to allow querying the block number from chain.
        *chain_monitor.next_update.write().await = Instant::now();

        let block = chain_monitor.current_block_number().await.unwrap();
        assert_eq!(block, NUM_BLOCKS);
    }
}
