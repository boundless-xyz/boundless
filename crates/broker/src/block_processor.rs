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

use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc,
};

use alloy::{
    eips::{BlockId, BlockNumberOrTag},
    network::AnyNetwork,
    primitives::{Address, B256},
    providers::Provider,
    rpc::types::Log,
};
use anyhow::Context;
use boundless_market::dynamic_gas_filler::PriorityMode;
use thiserror::Error;
use tokio::sync::mpsc;

use crate::{
    block_history::{BlockHistory, BlockHistoryEntry},
    errors::CodedError,
    impl_coded_debug,
};

#[derive(Error)]
pub enum BlockProcessorErr {
    #[error("{code} RPC error: {0:?}", code = self.code())]
    RpcErr(anyhow::Error),
    #[error("{code} Unexpected error: {0:?}", code = self.code())]
    UnexpectedErr(#[from] anyhow::Error),
}

impl_coded_debug!(BlockProcessorErr);

impl CodedError for BlockProcessorErr {
    fn code(&self) -> &str {
        match self {
            BlockProcessorErr::RpcErr(_) => "[B-BPR-400]",
            BlockProcessorErr::UnexpectedErr(_) => "[B-BPR-500]",
        }
    }
}

/// Lightweight block header passed from ChainMonitor to BlockProcessor per new block.
#[derive(Clone, Debug)]
pub(crate) struct BlockHeader {
    pub block_number: u64,
    pub block_hash: B256,
    pub parent_hash: B256,
    pub timestamp: u64,
    pub base_fee_per_gas: Option<u128>,
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

/// Processes blocks by fetching receipts and extracting market logs.
///
/// Called synchronously by `ChainMonitor` after each new block header is discovered.
/// For each block, fetches `eth_getBlockReceipts(N)` (single RPC call), filters market
/// contract logs, updates gas price estimates, and delivers `BlockUpdate` to
/// `MarketMonitor` via mpsc channel.
#[derive(Clone)]
pub(crate) struct BlockProcessor<ANP> {
    any_provider: Arc<ANP>,
    gas_priority_mode: Arc<tokio::sync::RwLock<PriorityMode>>,

    // Gas price atomic — lock-free reads for consumers.
    // Realistic gas prices (even 10_000 gwei) fit easily in u64.
    gas_price: Arc<AtomicU64>,

    // Block update delivery to MarketMonitor.
    block_update_tx: mpsc::Sender<BlockUpdate>,

    // Log filtering config.
    market_addr: Address,
    event_signatures: Vec<B256>,

    // Gas estimation state (survives supervisor restarts via Arc).
    block_history: Arc<std::sync::RwLock<BlockHistory>>,
}

impl<ANP> BlockProcessor<ANP>
where
    ANP: Provider<AnyNetwork> + Clone,
{
    pub fn new(
        any_provider: Arc<ANP>,
        gas_priority_mode: Arc<tokio::sync::RwLock<PriorityMode>>,
        market_addr: Address,
        event_signatures: Vec<B256>,
        block_update_tx: mpsc::Sender<BlockUpdate>,
        block_history_size: usize,
        initial_gas_price: u64,
    ) -> Self {
        Self {
            any_provider,
            gas_priority_mode,
            gas_price: Arc::new(AtomicU64::new(initial_gas_price)),
            block_update_tx,
            market_addr,
            event_signatures,
            block_history: Arc::new(std::sync::RwLock::new(BlockHistory::new(block_history_size))),
        }
    }

    /// Synchronous, lock-free read of the estimated max_fee_per_gas.
    pub fn current_gas_price(&self) -> u128 {
        self.gas_price.load(Ordering::Relaxed) as u128
    }

    /// Process a single block: fetch receipts, extract logs, update gas estimate.
    ///
    /// Called by `ChainMonitor` after discovering a new block. The header is passed
    /// in by the caller, so only one RPC call is needed: `eth_getBlockReceipts(N)`.
    pub(crate) async fn process_block(
        &self,
        block_number: u64,
        header: &BlockHeader,
    ) -> Result<(), BlockProcessorErr> {
        let receipts = self
            .any_provider
            .get_block_receipts(BlockId::Number(BlockNumberOrTag::Number(block_number)))
            .await
            .context("failed to fetch block receipts")
            .map_err(BlockProcessorErr::RpcErr)?
            .unwrap_or_default();

        let base_fee = header.base_fee_per_gas;

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
                block_hash: header.block_hash,
                parent_hash: header.parent_hash,
                base_fee_per_gas: base_fee,
                priority_fees,
            });
            history.estimate_gas_price(&mode)
        };
        if let Some(gp) = gas_price {
            self.gas_price.store(gp.min(u64::MAX as u128) as u64, Ordering::Relaxed);
        }

        tracing::trace!(
            block = block_number,
            logs = market_logs.len(),
            gas_price = self.gas_price.load(Ordering::Relaxed) as u128,
            "Processed block"
        );

        // Deliver filtered logs to MarketMonitor.
        let update = BlockUpdate {
            block_number,
            block_hash: header.block_hash,
            parent_hash: header.parent_hash,
            timestamp: header.timestamp,
            logs: market_logs,
        };

        if let Err(e) = self.block_update_tx.try_send(update) {
            match e {
                mpsc::error::TrySendError::Full(update) => {
                    // Backpressure: block until MarketMonitor catches up.
                    if self.block_update_tx.send(update).await.is_err() {
                        tracing::warn!("MarketMonitor receiver dropped, continuing");
                    }
                }
                mpsc::error::TrySendError::Closed(_) => {
                    tracing::warn!("MarketMonitor receiver dropped, continuing");
                }
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use alloy::{
        eips::BlockNumberOrTag,
        network::{AnyNetwork, EthereumWallet},
        node_bindings::Anvil,
        primitives::Address,
        providers::{ext::AnvilApi, Provider, ProviderBuilder},
        signers::local::PrivateKeySigner,
    };
    use boundless_market::dynamic_gas_filler::PriorityMode;
    use std::sync::Arc;
    use tokio::sync::mpsc;

    use super::*;

    #[tokio::test]
    async fn block_processor_smoke_test() {
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

        let processor = BlockProcessor::new(
            any_provider,
            gas_priority_mode,
            Address::ZERO,
            vec![],
            block_update_tx,
            20,
            0,
        );

        const NUM_BLOCKS: u64 = 3;
        provider.anvil_mine(Some(NUM_BLOCKS), Some(2)).await.unwrap();

        // Fetch headers and call process_block directly.
        for block_number in 1..=NUM_BLOCKS {
            let block = provider
                .get_block_by_number(BlockNumberOrTag::Number(block_number))
                .await
                .unwrap()
                .unwrap();
            let header = BlockHeader {
                block_number: block.header.number,
                block_hash: block.header.hash,
                parent_hash: block.header.parent_hash,
                timestamp: block.header.timestamp,
                base_fee_per_gas: block.header.base_fee_per_gas.map(|f| f as u128),
            };
            processor.process_block(block_number, &header).await.unwrap();
        }

        // We should receive NUM_BLOCKS BlockUpdate messages (empty logs since no market contract).
        let mut received = 0u64;
        while let Ok(update) = block_update_rx.try_recv() {
            assert!(update.logs.is_empty());
            received += 1;
        }
        assert_eq!(received, NUM_BLOCKS);
    }
}
