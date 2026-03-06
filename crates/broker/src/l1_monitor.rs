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

//! Experimental L1Monitor implementation.
//!
//! Replaces both `ChainMonitorService` and `MarketMonitor` with a single struct when
//! `--experimental-rpc` is set. A single polling loop fetches block receipts per block,
//! extracts market events, and updates the chain-head atomics read by `ChainMonitorApi`.

use std::{
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc,
    },
    time::Duration,
};

use crate::market_monitor::process_new_logs;
use crate::{
    block_history::{BlockHistory, BlockHistoryEntry},
    chain_monitor::{ChainHead, ChainMonitorApi},
    db::DbObj,
    errors::CodedError,
    impl_coded_debug,
    market_monitor::{
        process_log, process_order_submitted, process_request_fulfilled, process_request_locked,
        MarketEvent,
    },
    task::{RetryRes, RetryTask, SupervisorErr},
    OrderRequest, OrderStateChange,
};
use alloy::{
    eips::{BlockId, BlockNumberOrTag},
    network::{AnyNetwork, AnyTransactionReceipt, Ethereum},
    primitives::Address,
    providers::{utils::EIP1559_FEE_ESTIMATION_PAST_BLOCKS, Provider},
    rpc::types::{Filter, Log},
    sol_types::SolEvent,
};
use anyhow::{Context, Result};
use async_trait::async_trait;
use boundless_market::{
    contracts::{boundless_market::BoundlessMarketService, IBoundlessMarket},
    dynamic_gas_filler::PriorityMode,
};
use thiserror::Error;
use tokio::sync::{broadcast, mpsc, RwLock};
use tokio_util::sync::CancellationToken;

#[derive(Error)]
pub enum L1MonitorErr {
    #[error("{code} RPC error: {0:#}", code = self.code())]
    RpcErr(anyhow::Error),

    #[allow(dead_code)]
    #[error("{code} Log processing failed: {0:#}", code = self.code())]
    LogProcessingFailed(anyhow::Error),

    #[error("{code} Unexpected error: {0:#}", code = self.code())]
    UnexpectedErr(#[from] anyhow::Error),

    #[allow(dead_code)]
    #[error("{code} Receiver dropped", code = self.code())]
    ReceiverDropped,
}

impl_coded_debug!(L1MonitorErr);

impl CodedError for L1MonitorErr {
    fn code(&self) -> &str {
        match self {
            L1MonitorErr::RpcErr(_) => "[B-L1M-400]",
            L1MonitorErr::LogProcessingFailed(_) => "[B-L1M-501]",
            L1MonitorErr::UnexpectedErr(_) => "[B-L1M-500]",
            L1MonitorErr::ReceiverDropped => "[B-L1M-502]",
        }
    }
}

/// Experimental replacement for `ChainMonitorService` + `MarketMonitor`.
///
/// A single polling loop drives everything: one `eth_getBlockByNumber(Latest)` per tick,
/// then one `eth_getBlockReceipts` per unprocessed block to decode market events.
/// The chain-head atomics are updated on every tick and read synchronously by
/// [`ChainMonitorApi`] callers (e.g. `OrderPicker`, `OrderMonitor`).
#[derive(Clone)]
pub(crate) struct L1Monitor<P, ANP> {
    db: DbObj,
    provider: Arc<P>,
    any_provider: Arc<ANP>,

    market_addr: Address,
    prover_addr: Address,
    lookback_blocks: u64,
    chain_id: u64,

    poll_interval: Duration,
    block_time: u64,

    new_order_tx: mpsc::Sender<Box<OrderRequest>>,
    order_state_tx: broadcast::Sender<OrderStateChange>,

    head_block_number: Arc<AtomicU64>,
    head_block_timestamp: Arc<AtomicU64>,
    open_orders_found: Arc<AtomicBool>,

    gas_priority_mode: Arc<RwLock<PriorityMode>>,
    /// Ring buffer of recent block fee data for local EIP-1559 gas estimation.
    block_history: Arc<RwLock<BlockHistory>>,
    /// Estimated max_fee_per_gas (wei), updated on each processed block.
    gas_price: Arc<RwLock<u128>>,
}

impl<P, ANP> L1Monitor<P, ANP>
where
    P: Provider<Ethereum> + Send + Sync + 'static,
    ANP: Provider<AnyNetwork> + Send + Sync + 'static,
{
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn new(
        db: DbObj,
        provider: Arc<P>,
        any_provider: Arc<ANP>,
        market_addr: Address,
        prover_addr: Address,
        lookback_blocks: u64,
        chain_id: u64,
        new_order_tx: mpsc::Sender<Box<OrderRequest>>,
        order_state_tx: broadcast::Sender<OrderStateChange>,
        gas_priority_mode: Arc<RwLock<PriorityMode>>,
    ) -> Result<Self> {
        let initial_block = provider
            .get_block_by_number(BlockNumberOrTag::Latest)
            .await
            .context("Failed to get initial block")?
            .context("No block returned for initial fetch")?;

        let initial_number = initial_block.header.number;
        let initial_timestamp = initial_block.header.timestamp;

        // Fetch recent blocks for block-time sampling and to seed the gas history.
        // Uses the same window size as EIP1559_FEE_ESTIMATION_PAST_BLOCKS (10).
        let history_size = EIP1559_FEE_ESTIMATION_PAST_BLOCKS as usize;
        let sample_start = initial_number.saturating_sub(history_size as u64);
        let mut timestamps: Vec<u64> = Vec::new();
        let mut block_history = BlockHistory::new(history_size);

        for block_num in sample_start..initial_number {
            let block = provider
                .get_block_by_number(BlockNumberOrTag::Number(block_num))
                .await
                .with_context(|| format!("Failed to get block {block_num}"))?
                .with_context(|| format!("Missing block {block_num}"))?;

            timestamps.push(block.header.timestamp);
            block_history.push(BlockHistoryEntry {
                block_number: block.header.number,
                base_fee_per_gas: block.header.base_fee_per_gas.map(|f| f as u128),
                priority_fees: vec![], // no receipts at startup; falls back to EIP1559_MIN_PRIORITY_FEE
            });
        }
        // Also seed with the already-fetched latest block.
        timestamps.push(initial_block.header.timestamp);
        block_history.push(BlockHistoryEntry {
            block_number: initial_block.header.number,
            base_fee_per_gas: initial_block.header.base_fee_per_gas.map(|f| f as u128),
            priority_fees: vec![],
        });

        let mut block_times: Vec<u64> =
            timestamps.windows(2).map(|w| w[1].saturating_sub(w[0])).collect();
        block_times.sort_unstable();
        let block_time =
            if block_times.is_empty() { 0 } else { block_times[block_times.len() / 2] };

        let poll_interval = if block_time > 0 {
            Duration::from_secs(block_time).mul_f32(0.6)
        } else {
            Duration::from_secs(2)
        };

        let initial_gas_price = {
            let mode = gas_priority_mode.read().await.clone();
            block_history.estimate_gas_price(&mode).unwrap_or(0)
        };

        tracing::info!(
            "L1Monitor initialized at block {initial_number}, block_time={block_time}s, \
             poll_interval={poll_interval:?}, initial_gas_price={initial_gas_price}"
        );

        Ok(Self {
            db,
            provider,
            any_provider,
            market_addr,
            prover_addr,
            lookback_blocks,
            chain_id,
            poll_interval,
            block_time,
            new_order_tx,
            order_state_tx,
            head_block_number: Arc::new(AtomicU64::new(initial_number)),
            head_block_timestamp: Arc::new(AtomicU64::new(initial_timestamp)),
            open_orders_found: Arc::new(AtomicBool::new(false)),
            gas_priority_mode,
            block_history: Arc::new(RwLock::new(block_history)),
            gas_price: Arc::new(RwLock::new(initial_gas_price)),
        })
    }

    /// Returns the sampled block time (in seconds) from construction.
    pub(crate) fn block_time(&self) -> u64 {
        self.block_time
    }

    /// Walk `start_block..=end_block` using `get_logs`, automatically discovering the maximum
    /// chunk size accepted by the RPC provider via binary search on first failure.
    async fn adaptive_get_logs(
        &self,
        filter: Filter,
        start_block: u64,
        end_block: u64,
    ) -> Result<Vec<Log>, L1MonitorErr> {
        if start_block > end_block {
            return Ok(vec![]);
        }

        let total_range = end_block - start_block;
        let mut chunk_size = total_range.max(1);
        let mut chunk_found = false;
        let mut from = start_block;
        let mut all_logs: Vec<Log> = Vec::new();

        while from <= end_block {
            let to = (from + chunk_size).min(end_block);
            let ranged_filter = filter.clone().from_block(from).to_block(to);

            match self.provider.get_logs(&ranged_filter).await {
                Ok(logs) => {
                    all_logs.extend(logs);
                    chunk_found = true;
                    from = to + 1;
                }
                Err(e) => {
                    if chunk_found {
                        // Chunk size is already validated; unexpected failure.
                        return Err(L1MonitorErr::RpcErr(anyhow::anyhow!(
                            "get_logs failed for blocks {from}..{to}: {e:#}"
                        )));
                    }
                    if chunk_size <= 1 {
                        return Err(L1MonitorErr::RpcErr(anyhow::anyhow!(
                            "get_logs failed with chunk_size=1 for block {from}: {e:#}"
                        )));
                    }
                    tracing::debug!(
                        "get_logs failed for chunk_size={chunk_size} (blocks {from}..{to}), halving: {e:#}"
                    );
                    chunk_size /= 2;
                }
            }
        }

        Ok(all_logs)
    }

    async fn find_open_orders(&self) -> Result<(), L1MonitorErr> {
        let current_block = self.head_block_number.load(Ordering::Relaxed);
        let start_block = current_block.saturating_sub(self.lookback_blocks);

        tracing::info!("Searching for existing open orders: {start_block} - {current_block}");

        let market = BoundlessMarketService::new_for_broker(
            self.market_addr,
            self.provider.clone(),
            Address::ZERO,
        );

        let filter = Filter::new()
            .event_signature(IBoundlessMarket::RequestSubmitted::SIGNATURE_HASH)
            .from_block(start_block)
            .address(self.market_addr);

        let logs = self.adaptive_get_logs(filter, start_block, current_block).await?;

        process_new_logs(
            self.lookback_blocks,
            self.market_addr,
            &self.new_order_tx,
            self.chain_id,
            market,
            logs,
        )
        .await
        .map_err(|e| L1MonitorErr::RpcErr(anyhow::anyhow!(e)))?;

        Ok(())
    }

    async fn monitor_l1(&self, cancel_token: CancellationToken) -> Result<(), L1MonitorErr> {
        let mut last_processed = self.head_block_number.load(Ordering::Relaxed);
        let mut interval = tokio::time::interval(self.poll_interval);

        tracing::info!("L1Monitor polling loop started at block {last_processed}");

        loop {
            tokio::select! {
                _ = interval.tick() => {
                    let latest_block = match self
                        .provider
                        .get_block_by_number(BlockNumberOrTag::Latest)
                        .await
                    {
                        Ok(Some(block)) => block,
                        Ok(None) => {
                            tracing::warn!("No latest block returned from provider");
                            continue;
                        }
                        Err(e) => {
                            return Err(L1MonitorErr::RpcErr(
                                anyhow::anyhow!("Failed to get latest block: {e:#}"),
                            ));
                        }
                    };

                    let latest_number = latest_block.header.number;
                    let latest_timestamp = latest_block.header.timestamp;
                    self.head_block_number.store(latest_number, Ordering::Relaxed);
                    self.head_block_timestamp.store(latest_timestamp, Ordering::Relaxed);

                    if latest_number > last_processed {
                        tracing::debug!(
                            "Polling: latest block {latest_number} (last_processed={last_processed}, {} new blocks)",
                            latest_number - last_processed
                        );
                    }

                    // Walk every new block and process its receipts.
                    for block_num in (last_processed + 1)..=latest_number {
                        // Fetch the block header to get base_fee_per_gas for gas estimation.
                        // Reuse the already-fetched latest block in the common case (0 extra RPCs).
                        let base_fee_per_gas = if block_num == latest_number {
                            latest_block.header.base_fee_per_gas.map(|f| f as u128)
                        } else {
                            match self
                                .provider
                                .get_block_by_number(BlockNumberOrTag::Number(block_num))
                                .await
                            {
                                Ok(Some(b)) => b.header.base_fee_per_gas.map(|f| f as u128),
                                Ok(None) => {
                                    tracing::warn!("No block returned for {block_num}");
                                    None
                                }
                                Err(e) => {
                                    return Err(L1MonitorErr::RpcErr(anyhow::anyhow!(
                                        "Failed to get block header for {block_num}: {e:#}"
                                    )));
                                }
                            }
                        };

                        let block_id = BlockId::Number(block_num.into());
                        let receipts = match self
                            .any_provider
                            .get_block_receipts(block_id)
                            .await
                        {
                            Ok(Some(receipts)) => receipts,
                            Ok(None) => {
                                tracing::warn!("No receipts returned for block {block_num}");
                                continue;
                            }
                            Err(e) => {
                                return Err(L1MonitorErr::RpcErr(
                                    anyhow::anyhow!(
                                        "Failed to get block receipts for {block_num}: {e:#}"
                                    ),
                                ));
                            }
                        };

                        tracing::debug!("Fetched {} receipts for block {block_num}", receipts.len());

                        // Extract priority fees and update gas estimate.
                        let priority_fees: Vec<u128> = base_fee_per_gas
                            .map(|bf| {
                                receipts
                                    .iter()
                                    .map(|r| r.effective_gas_price.saturating_sub(bf))
                                    .collect()
                            })
                            .unwrap_or_default();

                        let mode = self.gas_priority_mode.read().await.clone();
                        let gas_price = {
                            let mut history = self.block_history.write().await;
                            history.push(BlockHistoryEntry {
                                block_number: block_num,
                                base_fee_per_gas,
                                priority_fees,
                            });
                            history.estimate_gas_price(&mode)
                        };
                        if let Some(gp) = gas_price {
                            *self.gas_price.write().await = gp;
                        }

                        if let Err(e) = self
                            .process_block_receipts(receipts, block_num)
                            .await
                        {
                            tracing::error!(
                                "Error processing receipts for block {block_num}: {e:?}"
                            );
                        }
                    }

                    last_processed = latest_number;
                }
                _ = cancel_token.cancelled() => {
                    tracing::info!("L1Monitor received cancellation, shutting down");
                    break;
                }
            }
        }

        Ok(())
    }

    /// Iterate all logs in `receipts`, filter by market address, decode and dispatch.
    async fn process_block_receipts(
        &self,
        receipts: Vec<AnyTransactionReceipt>,
        block_number: u64,
    ) -> Result<()> {
        let receipts_len = receipts.len();
        let mut event_count: u64 = 0;

        for receipt in receipts {
            for log in receipt.inner.logs() {
                if log.inner.address != self.market_addr {
                    continue;
                }
                let Some((event, _)) = process_log(log.clone()) else {
                    continue;
                };
                event_count += 1;
                match event {
                    MarketEvent::Submitted(event) => {
                        tracing::debug!(
                            "Block {block_number}: RequestSubmitted 0x{:x}",
                            event.requestId
                        );
                        if let Err(err) = process_order_submitted(
                            event,
                            self.provider.clone(),
                            self.market_addr,
                            self.chain_id,
                            &self.new_order_tx,
                        )
                        .await
                        {
                            tracing::error!("Failed to process RequestSubmitted: {err:?}");
                        }
                    }
                    MarketEvent::Locked(event) => {
                        tracing::debug!(
                            "Block {block_number}: RequestLocked 0x{:x} by 0x{:x}",
                            event.requestId,
                            event.prover
                        );
                        if let Err(err) = process_request_locked(
                            event,
                            block_number,
                            self.chain_id,
                            self.market_addr,
                            self.prover_addr,
                            &self.db,
                            &self.new_order_tx,
                            &self.order_state_tx,
                        )
                        .await
                        {
                            tracing::error!("Failed to process RequestLocked: {err:?}");
                        }
                    }
                    MarketEvent::Fulfilled(event) => {
                        tracing::debug!(
                            "Block {block_number}: RequestFulfilled 0x{:x}",
                            event.requestId
                        );
                        if let Err(err) = process_request_fulfilled(
                            event,
                            block_number,
                            &self.db,
                            &self.order_state_tx,
                        )
                        .await
                        {
                            tracing::error!("Failed to process RequestFulfilled: {err:?}");
                        }
                    }
                }
            }
        }

        tracing::trace!(
            "Processed block {block_number}: {event_count} market events from {receipts_len} receipts"
        );

        Ok(())
    }
}

#[async_trait]
impl<P, ANP> ChainMonitorApi for L1Monitor<P, ANP>
where
    P: Provider<Ethereum> + Send + Sync + 'static,
    ANP: Provider<AnyNetwork> + Send + Sync + 'static,
{
    async fn current_chain_head(&self) -> Result<ChainHead> {
        Ok(ChainHead {
            block_number: self.head_block_number.load(Ordering::Relaxed),
            block_timestamp: self.head_block_timestamp.load(Ordering::Relaxed),
        })
    }

    async fn current_gas_price(&self) -> Result<u128> {
        Ok(*self.gas_price.read().await)
    }
}

impl<P, ANP> RetryTask for L1Monitor<P, ANP>
where
    P: Provider<Ethereum> + Send + Sync + Clone + 'static,
    ANP: Provider<AnyNetwork> + Send + Sync + Clone + 'static,
{
    type Error = L1MonitorErr;

    fn spawn(&self, cancel_token: CancellationToken) -> RetryRes<Self::Error> {
        let self_clone = self.clone();

        Box::pin(async move {
            tracing::info!("Starting L1Monitor");

            if !self_clone.open_orders_found.load(Ordering::Relaxed) {
                self_clone.find_open_orders().await.map_err(SupervisorErr::Recover)?;
                self_clone.open_orders_found.store(true, Ordering::Relaxed);
            }

            self_clone.monitor_l1(cancel_token).await.map_err(SupervisorErr::Recover)?;

            Ok(())
        })
    }
}
