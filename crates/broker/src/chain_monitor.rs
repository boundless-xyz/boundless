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
    eips::{BlockId, BlockNumberOrTag},
    network::AnyNetwork,
    primitives::{Address, B256},
    providers::Provider,
    rpc::types::Log,
};
use alloy_chains::NamedChain;
use anyhow::Context;
use boundless_market::dynamic_gas_filler::PriorityMode;
use thiserror::Error;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::{
    block_history::{BlockHistory, BlockHistoryEntry},
    errors::CodedError,
    impl_coded_debug,
    task::{RetryRes, RetryTask, SupervisorErr},
};

#[derive(Error)]
pub enum ChainMonitorErr {
    #[error("{code} RPC error: {0:?}", code = self.code())]
    RpcErr(anyhow::Error),
    #[error("{code} Unexpected error: {0:?}", code = self.code())]
    UnexpectedErr(#[from] anyhow::Error),
}

impl_coded_debug!(ChainMonitorErr);

impl CodedError for ChainMonitorErr {
    fn code(&self) -> &str {
        match self {
            ChainMonitorErr::RpcErr(_) => "[B-CHM-400]",
            ChainMonitorErr::UnexpectedErr(_) => "[B-CHM-500]",
        }
    }
}

/// Snapshot of the chain head delivered to consumers.
#[derive(Clone, Debug, Copy)]
pub(crate) struct ChainHead {
    pub block_number: u64,
    pub block_timestamp: u64,
}

/// Per-block update delivered to MarketMonitor via mpsc.
#[derive(Clone, Debug)]
pub(crate) struct BlockUpdate {
    // Stored for future reorg detection; not yet read by consumers.
    #[allow(dead_code)]
    pub block_number: u64,
    #[allow(dead_code)]
    pub block_hash: B256,
    #[allow(dead_code)]
    pub parent_hash: B256,
    #[allow(dead_code)]
    pub timestamp: u64,
    /// Logs filtered by market contract address and event signatures.
    pub logs: Vec<Log>,
}

/// A service that proactively processes every block in order.
///
/// Per block: `eth_getBlockByNumber(N)` || `eth_getBlockReceipts(N)` in parallel.
/// Filtered market logs delivered to MarketMonitor via mpsc channel.
/// Gas price derived locally from a sliding window of block headers and receipt priority fees.
///
/// Consumer API (`current_block_number()`, `current_gas_price()`, `current_chain_head()`) is
/// synchronous and lock-free, backed by atomics.
#[derive(Clone)]
pub struct ChainMonitorService<P, ANP> {
    provider: Arc<P>,
    any_provider: Arc<ANP>,
    gas_priority_mode: Arc<tokio::sync::RwLock<PriorityMode>>,

    // Atomic state — lock-free reads for consumers.
    // Gas price stored as u64; realistic gas prices (even 10_000 gwei) fit easily.
    head_block_number: Arc<AtomicU64>,
    head_block_timestamp: Arc<AtomicU64>,
    gas_price: Arc<AtomicU64>,

    // Block update delivery to market monitor.
    block_update_tx: mpsc::Sender<BlockUpdate>,

    // Log filtering config.
    market_addr: Address,
    event_signatures: Vec<B256>,

    // Internal mutable state (survives Supervisor restarts via Arc).
    block_history: Arc<std::sync::RwLock<BlockHistory>>,
    last_processed_block: Arc<AtomicU64>,
}

impl<P, ANP> ChainMonitorService<P, ANP>
where
    P: Provider + Clone,
    ANP: Provider<AnyNetwork> + Clone,
{
    #[allow(clippy::too_many_arguments)]
    pub async fn new(
        provider: Arc<P>,
        any_provider: Arc<ANP>,
        gas_priority_mode: Arc<tokio::sync::RwLock<PriorityMode>>,
        market_addr: Address,
        event_signatures: Vec<B256>,
        block_update_tx: mpsc::Sender<BlockUpdate>,
        block_history_size: usize,
    ) -> anyhow::Result<Self> {
        // Fetch initial block to bootstrap atomics before returning.
        let block = provider
            .get_block_by_number(BlockNumberOrTag::Latest)
            .await
            .context("failed to fetch initial block")?
            .context("no block returned for latest")?;

        let base_fee = block.header.base_fee_per_gas.map(|f| f as u128);
        // Bootstrap gas estimate: base_fee * 2 + MIN_PRIORITY_FEE.
        let initial_gas_price = base_fee
            .map(|bf| bf.saturating_mul(2) + alloy::providers::utils::EIP1559_MIN_PRIORITY_FEE)
            .unwrap_or(0);

        Ok(Self {
            provider,
            any_provider,
            gas_priority_mode,
            head_block_number: Arc::new(AtomicU64::new(block.header.number)),
            head_block_timestamp: Arc::new(AtomicU64::new(block.header.timestamp)),
            gas_price: Arc::new(AtomicU64::new(initial_gas_price.min(u64::MAX as u128) as u64)),
            block_update_tx,
            market_addr,
            event_signatures,
            block_history: Arc::new(std::sync::RwLock::new(BlockHistory::new(block_history_size))),
            last_processed_block: Arc::new(AtomicU64::new(block.header.number)),
        })
    }

    /// Synchronous, lock-free read of the latest processed block number.
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
        self.gas_price.load(Ordering::Relaxed) as u128
    }

    /// Process a single block: fetch header + receipts in parallel, extract logs, update state.
    async fn process_block(&self, block_number: u64) -> Result<(), ChainMonitorErr> {
        // Fetch header and receipts in parallel.
        let (block_res, receipts_res) = tokio::join!(
            self.provider.get_block_by_number(BlockNumberOrTag::Number(block_number)),
            self.any_provider
                .get_block_receipts(BlockId::Number(BlockNumberOrTag::Number(block_number)))
        );

        let block = block_res
            .context("failed to fetch block")
            .map_err(ChainMonitorErr::RpcErr)?
            .with_context(|| format!("no block returned for {block_number}"))
            .map_err(ChainMonitorErr::UnexpectedErr)?;

        let receipts = receipts_res
            .context("failed to fetch block receipts")
            .map_err(ChainMonitorErr::RpcErr)?
            .unwrap_or_default();

        let base_fee = block.header.base_fee_per_gas.map(|f| f as u128);

        // Extract priority fees from all receipts.
        let priority_fees: Vec<u128> = receipts
            .iter()
            .filter_map(|r| {
                let effective = r.effective_gas_price;
                base_fee.map(|bf| effective.saturating_sub(bf))
            })
            .collect();

        // Filter logs: match market_addr + event signatures.
        let market_logs: Vec<Log> = receipts
            .iter()
            .flat_map(|r| r.logs().iter())
            .filter(|log| {
                log.address() == self.market_addr
                    && log.topic0().map(|t| self.event_signatures.contains(t)).unwrap_or(false)
            })
            .cloned()
            .collect();

        // Compute gas estimate: acquire mode, update history, estimate — all without holding
        // std::sync::RwLock across an await point.
        let gas_price = {
            let mode = self.gas_priority_mode.read().await.clone();
            let mut history = self.block_history.write().unwrap();
            history.push(BlockHistoryEntry {
                block_number,
                block_hash: block.header.hash,
                parent_hash: block.header.parent_hash,
                base_fee_per_gas: base_fee,
                priority_fees,
            });
            history.estimate_gas_price(&mode)
        };
        if let Some(gp) = gas_price {
            self.gas_price.store(gp.min(u64::MAX as u128) as u64, Ordering::Relaxed);
        }

        // Update head atomics.
        self.head_block_number.store(block_number, Ordering::Relaxed);
        self.head_block_timestamp.store(block.header.timestamp, Ordering::Relaxed);
        self.last_processed_block.store(block_number, Ordering::Relaxed);

        tracing::trace!(
            block = block_number,
            logs = market_logs.len(),
            gas_price = self.gas_price.load(Ordering::Relaxed) as u128,
            "Processed block"
        );

        // Deliver filtered logs to market monitor.
        let update = BlockUpdate {
            block_number,
            block_hash: block.header.hash,
            parent_hash: block.header.parent_hash,
            timestamp: block.header.timestamp,
            logs: market_logs,
        };

        if let Err(e) = self.block_update_tx.try_send(update) {
            match e {
                mpsc::error::TrySendError::Full(update) => {
                    // Backpressure: block until market_monitor catches up.
                    if self.block_update_tx.send(update).await.is_err() {
                        tracing::warn!("Market monitor receiver dropped, continuing");
                    }
                }
                mpsc::error::TrySendError::Closed(_) => {
                    tracing::warn!("Market monitor receiver dropped, continuing");
                }
            }
        }

        Ok(())
    }
}

impl<P, ANP> RetryTask for ChainMonitorService<P, ANP>
where
    P: Provider + 'static + Clone,
    ANP: Provider<AnyNetwork> + 'static + Clone,
{
    type Error = ChainMonitorErr;

    fn spawn(&self, cancel_token: CancellationToken) -> RetryRes<Self::Error> {
        let self_clone = self.clone();

        Box::pin(async move {
            tracing::info!("Starting ChainMonitor sequencer");

            let chain_id = self_clone
                .provider
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

            let mut next_block = self_clone.last_processed_block.load(Ordering::Relaxed) + 1;
            let mut ticker = tokio::time::interval(poll_interval);
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

            loop {
                tokio::select! {
                    _ = ticker.tick() => {
                        // Fetch latest head to know how far to process.
                        let latest_block = self_clone.provider
                            .get_block_by_number(BlockNumberOrTag::Latest)
                            .await
                            .context("failed to fetch latest block")
                            .map_err(ChainMonitorErr::RpcErr)
                            .map_err(SupervisorErr::Recover)?
                            .context("no block returned for latest")
                            .map_err(ChainMonitorErr::UnexpectedErr)
                            .map_err(SupervisorErr::Recover)?;

                        let latest_number = latest_block.header.number;

                        // Process all blocks from next_block to latest in order.
                        while next_block <= latest_number {
                            self_clone.process_block(next_block).await
                                .map_err(SupervisorErr::Recover)?;
                            next_block += 1;
                        }
                    }
                    _ = cancel_token.cancelled() => {
                        tracing::debug!("Chain monitor sequencer shutting down");
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
        primitives::Address,
        providers::{ext::AnvilApi, ProviderBuilder},
        signers::local::PrivateKeySigner,
        sol_types::SolEvent,
    };
    use boundless_market::{contracts::IBoundlessMarket, dynamic_gas_filler::PriorityMode};
    use std::sync::Arc;
    use tokio::sync::mpsc;

    use super::*;

    async fn make_chain_monitor(
        endpoint: &str,
        signer: PrivateKeySigner,
    ) -> (
        Arc<ChainMonitorService<impl Provider, impl Provider<AnyNetwork>>>,
        mpsc::Receiver<BlockUpdate>,
    ) {
        let provider = Arc::new(
            ProviderBuilder::new()
                .wallet(EthereumWallet::from(signer.clone()))
                .connect(endpoint)
                .await
                .unwrap(),
        );

        let any_provider = Arc::new(
            ProviderBuilder::new()
                .disable_recommended_fillers()
                .network::<AnyNetwork>()
                .connect(endpoint)
                .await
                .unwrap(),
        );

        let gas_priority_mode = Arc::new(tokio::sync::RwLock::new(PriorityMode::default()));
        let (block_update_tx, block_update_rx) = mpsc::channel(64);

        let event_signatures = vec![
            IBoundlessMarket::RequestSubmitted::SIGNATURE_HASH,
            IBoundlessMarket::RequestLocked::SIGNATURE_HASH,
            IBoundlessMarket::RequestFulfilled::SIGNATURE_HASH,
        ];

        let chain_monitor = Arc::new(
            ChainMonitorService::new(
                provider,
                any_provider,
                gas_priority_mode,
                Address::ZERO,
                event_signatures,
                block_update_tx,
                20,
            )
            .await
            .unwrap(),
        );

        (chain_monitor, block_update_rx)
    }

    #[tokio::test]
    async fn chain_monitor_smoke_test() {
        // Using an unknown chain ID to use default 2s polling time.
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
        let (block_update_tx, mut block_update_rx) = mpsc::channel(64);

        let chain_monitor = Arc::new(
            ChainMonitorService::new(
                provider.clone(),
                any_provider,
                gas_priority_mode,
                Address::ZERO,
                vec![],
                block_update_tx,
                20,
            )
            .await
            .unwrap(),
        );

        // Block number is 0 right after construction (no blocks mined yet).
        assert_eq!(chain_monitor.current_block_number(), 0);

        // Gas price should be bootstrapped to a non-zero value.
        // (On anvil with EIP-1559 disabled, base_fee might be 0 → gas_price = MIN_PRIORITY_FEE)

        let cancel = CancellationToken::new();
        tokio::spawn({
            let cm = chain_monitor.clone();
            let ct = cancel.clone();
            async move { cm.spawn(ct).await }
        });

        const NUM_BLOCKS: u64 = 3;
        provider.anvil_mine(Some(NUM_BLOCKS), Some(2)).await.unwrap();

        // Wait for the sequencer to process the blocks.
        let timeout = std::time::Duration::from_secs(10);
        let start = std::time::Instant::now();
        loop {
            if chain_monitor.current_block_number() >= NUM_BLOCKS {
                break;
            }
            if start.elapsed() > timeout {
                panic!("Timed out waiting for chain monitor to process blocks");
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }

        assert_eq!(chain_monitor.current_block_number(), NUM_BLOCKS);

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
