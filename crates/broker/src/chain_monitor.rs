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

use std::{
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    time::Duration,
};

use alloy::{
    eips::BlockNumberOrTag,
    network::AnyNetwork,
    primitives::{Address, B256},
    providers::Provider,
};
use alloy_chains::NamedChain;

use anyhow::Context;
use boundless_market::dynamic_gas_filler::PriorityMode;
use thiserror::Error;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::{
    block_processor::{BlockHeader, BlockProcessor, BlockProcessorErr, BlockUpdate},
    errors::CodedError,
    impl_coded_debug,
    task::{RetryRes, RetryTask, SupervisorErr},
};

#[derive(Error)]
pub enum ChainMonitorErr {
    #[error("{code} RPC error: {0:?}", code = self.code())]
    RpcErr(anyhow::Error),
    #[error("{code} Block processing error: {0:?}", code = self.code())]
    BlockProcessor(BlockProcessorErr),
    #[error("{code} Unexpected error: {0:?}", code = self.code())]
    UnexpectedErr(#[from] anyhow::Error),
}

impl_coded_debug!(ChainMonitorErr);

impl CodedError for ChainMonitorErr {
    fn code(&self) -> &str {
        match self {
            ChainMonitorErr::RpcErr(_) => "[B-CHM-400]",
            ChainMonitorErr::BlockProcessor(e) => e.code(), // preserves [B-BPR-*]
            ChainMonitorErr::UnexpectedErr(_) => "[B-CHM-500]",
        }
    }
}

/// Snapshot of the chain head for synchronous consumers.
#[derive(Clone, Debug, Copy)]
pub(crate) struct ChainHead {
    pub block_number: u64,
    pub block_timestamp: u64,
}

/// Follows the chain head, processes blocks, and exposes chain state to consumers.
///
/// Owns the polling loop (`eth_getBlockByNumber(Latest)` on a timer) and calls
/// `BlockProcessor::process_block()` synchronously for each new block. This ensures
/// `eth_getBlockReceipts(N)` is only fetched once we know a new block exists.
///
/// Consumers interact with a single `Arc<ChainMonitor>` rather than separate
/// `Arc<ChainHeadFollower>` and `Arc<BlockProcessor>` references.
pub(crate) struct ChainMonitor<P, ANP> {
    provider: Arc<P>,

    // Atomic state — lock-free reads for consumers (MarketMonitor, OrderPicker, OrderMonitor).
    head_block_number: Arc<AtomicU64>,
    head_block_timestamp: Arc<AtomicU64>,

    // Tracks the last block delivered, survives supervisor restarts via Arc.
    last_delivered_block: Arc<AtomicU64>,

    // Block processing (receipts, gas estimation, log filtering).
    processor: BlockProcessor<ANP>,
}

impl<P, ANP> ChainMonitor<P, ANP>
where
    P: Provider + Clone,
    ANP: Provider<AnyNetwork> + Clone,
{
    /// Create a new `ChainMonitor`, bootstrapping initial state from the latest block.
    ///
    /// Returns `(ChainMonitor, mpsc::Receiver<BlockUpdate>)`. The receiver must be given
    /// to `MarketMonitor` to receive per-block log updates.
    pub async fn new(
        provider: Arc<P>,
        any_provider: Arc<ANP>,
        gas_priority_mode: Arc<tokio::sync::RwLock<PriorityMode>>,
        market_addr: Address,
        event_signatures: Vec<B256>,
        block_history_size: usize,
        block_update_capacity: usize,
    ) -> anyhow::Result<(Self, mpsc::Receiver<BlockUpdate>)> {
        let block = provider
            .get_block_by_number(BlockNumberOrTag::Latest)
            .await
            .context("failed to fetch initial block")?
            .context("no block returned for latest")?;

        let initial_base_fee = block.header.base_fee_per_gas.map(|f| f as u128);
        let initial_gas_price = initial_base_fee
            .map(|bf| bf.saturating_mul(2) + alloy::providers::utils::EIP1559_MIN_PRIORITY_FEE)
            .unwrap_or(0)
            .min(u64::MAX as u128) as u64;

        let (block_update_tx, block_update_rx) = mpsc::channel(block_update_capacity);

        let processor = BlockProcessor::new(
            any_provider,
            gas_priority_mode,
            market_addr,
            event_signatures,
            block_update_tx,
            block_history_size,
            initial_gas_price,
        );

        let monitor = Self {
            provider,
            head_block_number: Arc::new(AtomicU64::new(block.header.number)),
            head_block_timestamp: Arc::new(AtomicU64::new(block.header.timestamp)),
            last_delivered_block: Arc::new(AtomicU64::new(block.header.number)),
            processor,
        };

        Ok((monitor, block_update_rx))
    }

    /// Synchronous, lock-free read of the latest delivered block number.
    pub fn current_block_number(&self) -> u64 {
        self.head_block_number.load(Ordering::Relaxed)
    }

    /// Synchronous, lock-free read of the chain head (block number + timestamp).
    pub(crate) fn current_chain_head(&self) -> ChainHead {
        ChainHead {
            block_number: self.head_block_number.load(Ordering::Relaxed),
            block_timestamp: self.head_block_timestamp.load(Ordering::Relaxed),
        }
    }

    /// Synchronous, lock-free read of the estimated max_fee_per_gas.
    pub fn current_gas_price(&self) -> u128 {
        self.processor.current_gas_price()
    }
}

impl<P, ANP> RetryTask for ChainMonitor<P, ANP>
where
    P: Provider + 'static + Clone,
    ANP: Provider<AnyNetwork> + 'static + Clone,
{
    type Error = ChainMonitorErr;

    fn spawn(&self, cancel_token: CancellationToken) -> RetryRes<Self::Error> {
        let provider = self.provider.clone();
        let processor = self.processor.clone();
        let head_block_number = self.head_block_number.clone();
        let head_block_timestamp = self.head_block_timestamp.clone();
        let last_delivered_block = self.last_delivered_block.clone();

        Box::pin(async move {
            tracing::info!("Starting ChainMonitor");

            let chain_id = provider
                .get_chain_id()
                .await
                .context("failed to get chain ID")
                .map_err(ChainMonitorErr::UnexpectedErr)
                .map_err(SupervisorErr::Recover)?;

            let poll_interval = NamedChain::try_from(chain_id)
                .ok()
                .and_then(|chain| chain.average_blocktime_hint())
                .map(|block_time| block_time.mul_f32(0.6))
                .unwrap_or(Duration::from_secs(2));

            let mut next_block = last_delivered_block.load(Ordering::Relaxed) + 1;
            let mut ticker = tokio::time::interval(poll_interval);
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

            loop {
                tokio::select! {
                    _ = ticker.tick() => {
                        // Fetch latest head to know how far to walk forward.
                        let latest_block = provider
                            .get_block_by_number(BlockNumberOrTag::Latest)
                            .await
                            .context("failed to fetch latest block")
                            .map_err(ChainMonitorErr::RpcErr)
                            .map_err(SupervisorErr::Recover)?
                            .context("no block returned for latest")
                            .map_err(ChainMonitorErr::UnexpectedErr)
                            .map_err(SupervisorErr::Recover)?;

                        let latest_number = latest_block.header.number;

                        // Walk [next_block, latest_number] in order, fetching and processing
                        // each block. For the common case (one new block), reuse the already-
                        // fetched latest block to avoid a redundant RPC call.
                        while next_block <= latest_number {
                            let block = if next_block == latest_number {
                                // Reuse the already-fetched block.
                                latest_block.clone()
                            } else {
                                // Catch-up: fetch an intermediate block.
                                provider
                                    .get_block_by_number(BlockNumberOrTag::Number(next_block))
                                    .await
                                    .context("failed to fetch block")
                                    .map_err(ChainMonitorErr::RpcErr)
                                    .map_err(SupervisorErr::Recover)?
                                    .with_context(|| format!("no block returned for {next_block}"))
                                    .map_err(ChainMonitorErr::UnexpectedErr)
                                    .map_err(SupervisorErr::Recover)?
                            };

                            let header = BlockHeader {
                                block_number: block.header.number,
                                block_hash: block.header.hash,
                                parent_hash: block.header.parent_hash,
                                timestamp: block.header.timestamp,
                                base_fee_per_gas: block.header.base_fee_per_gas.map(|f| f as u128),
                            };

                            // Update atomics so synchronous consumers get fresh data.
                            head_block_number.store(header.block_number, Ordering::Relaxed);
                            head_block_timestamp.store(header.timestamp, Ordering::Relaxed);
                            last_delivered_block.store(header.block_number, Ordering::Relaxed);

                            // Process the block synchronously: fetch receipts, update gas, send logs.
                            processor
                                .process_block(header.block_number, &header)
                                .await
                                .map_err(ChainMonitorErr::BlockProcessor)
                                .map_err(SupervisorErr::Recover)?;

                            next_block += 1;
                        }
                    }
                    _ = cancel_token.cancelled() => {
                        tracing::debug!("ChainMonitor shutting down");
                        break;
                    }
                }
            }

            Ok(())
        })
    }
}

#[cfg(test)]
mod tests {
    use alloy::{
        network::{AnyNetwork, EthereumWallet},
        node_bindings::Anvil,
        providers::{ext::AnvilApi, ProviderBuilder},
        signers::local::PrivateKeySigner,
    };
    use std::sync::Arc;

    use super::*;

    #[tokio::test]
    async fn chain_monitor_smoke_test() {
        // Use an unknown chain ID to force the default 2s poll interval.
        let anvil = Anvil::new().chain_id(888833888).spawn();
        let signer: PrivateKeySigner = anvil.keys()[0].clone().into();

        let provider = Arc::new(
            ProviderBuilder::new()
                .wallet(EthereumWallet::from(signer.clone()))
                .connect(&anvil.endpoint())
                .await
                .unwrap(),
        );
        let any_provider = Arc::new(
            ProviderBuilder::new()
                .disable_recommended_fillers()
                .network::<AnyNetwork>()
                .connect(&anvil.endpoint())
                .await
                .unwrap(),
        );

        let gas_priority_mode = Arc::new(tokio::sync::RwLock::new(PriorityMode::default()));

        let (monitor, mut block_update_rx) = ChainMonitor::new(
            provider.clone(),
            any_provider,
            gas_priority_mode,
            alloy::primitives::Address::ZERO,
            vec![],
            20,
            64,
        )
        .await
        .unwrap();

        // Block number is 0 right after construction (no blocks mined yet).
        assert_eq!(monitor.current_block_number(), 0);

        let monitor = Arc::new(monitor);

        let cancel = CancellationToken::new();
        tokio::spawn({
            let m = monitor.clone();
            let ct = cancel.clone();
            async move { m.spawn(ct).await }
        });

        const NUM_BLOCKS: u64 = 3;
        provider.anvil_mine(Some(NUM_BLOCKS), Some(2)).await.unwrap();

        // Wait for ChainMonitor to process all blocks.
        let timeout = std::time::Duration::from_secs(10);
        let start = std::time::Instant::now();
        loop {
            if monitor.current_block_number() >= NUM_BLOCKS {
                break;
            }
            if start.elapsed() > timeout {
                panic!("Timed out waiting for ChainMonitor to process blocks");
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }

        assert_eq!(monitor.current_block_number(), NUM_BLOCKS);

        // We should receive NUM_BLOCKS BlockUpdate messages (empty logs since no market contract).
        let mut received = 0u64;
        while let Ok(update) = block_update_rx.try_recv() {
            assert!(update.logs.is_empty());
            received += 1;
        }
        assert_eq!(received, NUM_BLOCKS);

        cancel.cancel();
    }
}
