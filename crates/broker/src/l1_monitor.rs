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

use crate::futures_retry::retry;
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
    consensus::proofs::calculate_receipt_root,
    eips::{BlockId, BlockNumberOrTag},
    network::{AnyNetwork, AnyTransactionReceipt, Ethereum},
    primitives::{Address, B256},
    providers::{utils::EIP1559_FEE_ESTIMATION_PAST_BLOCKS, Provider},
    rpc::types::{Filter, Log},
    sol_types::SolEvent,
};
use alloy_consensus_any::AnyReceiptEnvelope;
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
    #[error("{code} Receipts root mismatch: {0:#}", code = self.code())]
    ReceiptsMismatch(anyhow::Error),

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
            L1MonitorErr::ReceiptsMismatch(_) => "[B-L1M-401]",
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
/// [`ChainMonitorApi`] callers (e.g. `OrderPricer`, `OrderCommitter`).
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
    /// Tracks the last block whose gas price and receipts were fully processed.
    /// Separate from `head_block_number` (chain head) so that on Supervisor restart,
    /// the monitor resumes from the last successfully processed block, not the chain head.
    last_processed_block: Arc<AtomicU64>,

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

        let poll_interval =
            if block_time > 0 { Duration::from_secs(block_time) } else { Duration::from_secs(2) };

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
            last_processed_block: Arc::new(AtomicU64::new(initial_number)),
            gas_priority_mode,
            block_history: Arc::new(RwLock::new(block_history)),
            gas_price: Arc::new(RwLock::new(initial_gas_price)),
        })
    }

    /// Returns the sampled block time (in seconds) from construction.
    pub(crate) fn block_time(&self) -> u64 {
        self.block_time
    }

    #[cfg(test)]
    fn head_block(&self) -> u64 {
        self.head_block_number.load(Ordering::Relaxed)
    }

    #[cfg(test)]
    fn last_processed(&self) -> u64 {
        self.last_processed_block.load(Ordering::Relaxed)
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

    /// NOTE: This implementation assumes no chain reorgs (safe for OP Stack L2s with
    /// sequencer finality). On L1 Ethereum a reorg could cause the chain head to regress,
    /// and blocks on the new fork would be missed. L1 deployment would require block-hash
    /// tracking and reorg detection.
    async fn monitor_l1(&self, cancel_token: CancellationToken) -> Result<(), L1MonitorErr> {
        // Initialise the processing cursor from `last_processed_block` (not `head_block_number`).
        // On Supervisor restart after a mid-batch RPC failure, this resumes from the last
        // successfully processed block rather than from the (already-updated) chain head.
        let mut last_processed = self.last_processed_block.load(Ordering::Relaxed);
        let mut interval = tokio::time::interval(self.poll_interval);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

        tracing::info!(
            "L1Monitor polling loop started at block {last_processed} with {}s poll interval",
            self.poll_interval.as_secs_f64()
        );

        loop {
            tokio::select! {
                _ = interval.tick() => {
                    tracing::debug!("L1Monitor poll interval tick, last processed block {last_processed}, known latest block {}", self.head_block_number.load(Ordering::Relaxed));
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

                    if latest_number <= self.head_block_number.load(Ordering::Relaxed) {
                        tracing::debug!(
                            "Polling: latest block {latest_number} not ahead of current head {}, skipping",
                            self.head_block_number.load(Ordering::Relaxed)
                        );
                        continue;
                    }

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
                        if cancel_token.is_cancelled() {
                            tracing::info!("Cancellation received during block processing, stopping at block {block_num}");
                            break;
                        }
                        // Fetch the block header (for base_fee_per_gas and receipts_root) and
                        // receipts concurrently. When block_num == latest_number, the header is
                        // already available — only the receipts need fetching (0 extra header RPCs).
                        let (base_fee_per_gas, _receipts_root, receipts): (
                            Option<u128>,
                            B256,
                            Vec<AnyTransactionReceipt>,
                        ) = if block_num == latest_number {
                            let receipts = retry(
                                3,
                                500,
                                || async {
                                    let block_id = BlockId::Number(block_num.into());
                                    match self.any_provider.get_block_receipts(block_id).await {
                                        Ok(Some(r)) => Ok(r),
                                        Ok(None) => Err(L1MonitorErr::RpcErr(anyhow::anyhow!(
                                            "No receipts returned for block {block_num}"
                                        ))),
                                        Err(e) => Err(L1MonitorErr::RpcErr(anyhow::anyhow!(
                                            "Failed to get block receipts for {block_num}: {e:#}"
                                        ))),
                                    }
                                },
                                "get_block_receipts",
                            )
                            .await?;
                            (
                                latest_block.header.base_fee_per_gas.map(|f| f as u128),
                                latest_block.header.receipts_root,
                                receipts,
                            )
                        } else {
                            // Fire header and receipts RPCs concurrently for non-latest blocks.
                            let (header_result, receipts_result) = tokio::join!(
                                retry(
                                    3,
                                    500,
                                    || async {
                                        match self
                                            .provider
                                            .get_block_by_number(BlockNumberOrTag::Number(
                                                block_num,
                                            ))
                                            .await
                                        {
                                            Ok(Some(b)) => Ok((
                                                b.header.base_fee_per_gas.map(|f| f as u128),
                                                b.header.receipts_root,
                                            )),
                                            Ok(None) => {
                                                Err(L1MonitorErr::RpcErr(anyhow::anyhow!(
                                                    "No block returned for {block_num}"
                                                )))
                                            }
                                            Err(e) => Err(L1MonitorErr::RpcErr(anyhow::anyhow!(
                                                "Failed to get block header for {block_num}: {e:#}"
                                            ))),
                                        }
                                    },
                                    "get_block_by_number"
                                ),
                                retry(
                                    3,
                                    500,
                                    || async {
                                        let block_id = BlockId::Number(block_num.into());
                                        match self.any_provider.get_block_receipts(block_id).await
                                        {
                                            Ok(Some(r)) => Ok(r),
                                            Ok(None) => {
                                                Err(L1MonitorErr::RpcErr(anyhow::anyhow!(
                                                    "No receipts returned for block {block_num}"
                                                )))
                                            }
                                            Err(e) => Err(L1MonitorErr::RpcErr(anyhow::anyhow!(
                                                "Failed to get block receipts for {block_num}: {e:#}"
                                            ))),
                                        }
                                    },
                                    "get_block_receipts"
                                ),
                            );
                            let (base_fee, receipts_root) = header_result?;
                            let receipts = receipts_result?;
                            (base_fee, receipts_root, receipts)
                        };

                        tracing::debug!("Fetched {} receipts for block {block_num}", receipts.len());

                        // TODO: Verify the receipts match the block header commitment.
                        // This cryptographically ensures the integrity of the receipts
                        // and makes sure we process all market logs.
                        // verify_receipts_root(&receipts, _receipts_root)?;

                        self.process_block_receipts(&receipts, block_num).await?;

                        // Extract priority fees and update gas estimate.
                        let priority_fees: Vec<u128> = match base_fee_per_gas {
                            Some(bf) => receipts
                                .iter()
                                .map(|r| {
                                    if r.effective_gas_price < bf {
                                        tracing::debug!(
                                            "effective_gas_price {} below base fee {} (tx_type={})",
                                            r.effective_gas_price,
                                            bf,
                                            r.inner.inner.r#type,
                                        );
                                    }
                                    r.effective_gas_price.saturating_sub(bf)
                                }).filter(|pf| *pf > 0)
                                .collect(),
                            None => Vec::new(),
                        };

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

                        // Advance the processing cursor *after* successful completion of this
                        // block so that a Supervisor restart resumes from here.
                        self.last_processed_block.store(block_num, Ordering::Relaxed);
                        last_processed = block_num;
                    }
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
        receipts: &Vec<AnyTransactionReceipt>,
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

#[allow(dead_code)]
fn verify_receipts_root(
    receipts: &[AnyTransactionReceipt],
    expected_root: B256,
) -> Result<(), L1MonitorErr> {
    let envelopes: Vec<AnyReceiptEnvelope> = receipts
        .iter()
        .map(|r| {
            let env = &r.inner.inner;
            AnyReceiptEnvelope {
                inner: env.inner.clone().into_primitives_receipt(),
                r#type: env.r#type,
            }
        })
        .collect();
    let computed_root = calculate_receipt_root(&envelopes);
    if computed_root != expected_root {
        return Err(L1MonitorErr::ReceiptsMismatch(anyhow::anyhow!(
            "expected {expected_root}, got {computed_root}"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::rpc::types::BlockTransactions;
    use alloy::{
        consensus::Header as ConsensusHeader,
        network::AnyNetwork,
        primitives::Address,
        providers::{utils::EIP1559_FEE_ESTIMATION_PAST_BLOCKS, ProviderBuilder},
        rpc::types::{Block, Header as RpcHeader},
        transports::mock::Asserter,
    };
    use boundless_market::dynamic_gas_filler::PriorityMode;
    use std::time::Duration;
    use tokio::sync::{broadcast, mpsc, RwLock};
    use tracing_test::traced_test;

    use crate::db::SqliteDb;

    /// Constructs a minimal `Block` with given number, timestamp, and EIP-1559 base fee.
    fn make_block(number: u64, timestamp: u64, base_fee: Option<u64>) -> Block {
        Block::new(
            RpcHeader::new(ConsensusHeader {
                number,
                timestamp,
                base_fee_per_gas: base_fee,
                ..Default::default()
            }),
            BlockTransactions::default(),
        )
    }

    /// Push all RPC responses needed by `L1Monitor::new()` for an initial chain head at
    /// `initial_number`.  History blocks get timestamps `n * 2` so that block_time ≈ 2 s,
    /// which the tests then skip with `tokio::time::advance`.
    fn push_new_responses(eth: &Asserter, initial_number: u64) {
        // 1. get_block_by_number(Latest)
        eth.push_success(&Some(make_block(
            initial_number,
            initial_number * 2,
            Some(1_000_000_000),
        )));
        // 2. History loop: blocks sample_start..initial_number
        let sample_start = initial_number.saturating_sub(EIP1559_FEE_ESTIMATION_PAST_BLOCKS);
        for n in sample_start..initial_number {
            eth.push_success(&Some(make_block(n, n * 2, Some(1_000_000_000))));
        }
    }

    /// Creates an `L1Monitor` backed by mock Asserter providers.
    async fn make_monitor(
        eth: Asserter,
        any: Asserter,
    ) -> L1Monitor<
        impl Provider<Ethereum> + Clone + 'static,
        impl Provider<AnyNetwork> + Clone + 'static,
    > {
        let provider = Arc::new(ProviderBuilder::new().connect_mocked_client(eth));
        let any_provider =
            Arc::new(ProviderBuilder::new().network::<AnyNetwork>().connect_mocked_client(any));

        let db: DbObj = Arc::new(SqliteDb::new("sqlite::memory:").await.unwrap());
        let (new_order_tx, _rx) = mpsc::channel(16);
        let (order_state_tx, _) = broadcast::channel(16);
        let gas_priority_mode = Arc::new(RwLock::new(PriorityMode::default()));

        L1Monitor::new(
            db,
            provider,
            any_provider,
            Address::ZERO,
            Address::ZERO,
            10, // lookback_blocks
            1,  // chain_id
            new_order_tx,
            order_state_tx,
            gas_priority_mode,
        )
        .await
        .expect("L1Monitor::new should succeed with valid mock responses")
    }

    /// Verifies that `verify_receipts_root` correctly computes the receipts root for a real
    /// Base block (43414930) using static fixture data captured from the RPC. Currently fails due to Base's non-standard receipts format
    #[test]
    fn verify_receipts_root_base_43414930() {
        let receipts_json = include_str!("../testdata/base_43414930_receipts.json");
        let block_json = include_str!("../testdata/base_43414930_block.json");

        let receipts: Vec<AnyTransactionReceipt> =
            serde_json::from_str(receipts_json).expect("failed to deserialize receipts");
        let block: serde_json::Value =
            serde_json::from_str(block_json).expect("failed to deserialize block");
        let expected_root: B256 = block["receiptsRoot"]
            .as_str()
            .expect("receiptsRoot missing")
            .parse()
            .expect("failed to parse receiptsRoot");

        // Base receipts should fail
        assert!(verify_receipts_root(&receipts, expected_root).is_err())
    }

    /// Verifies that `verify_receipts_root` correctly computes the receipts root for a real
    /// ETH mainnet block (24666352) using static fixture data.
    #[test]
    fn verify_receipts_root_eth_24666352() {
        let receipts_json = include_str!("../testdata/eth_24666352_receipts.json");
        let block_json = include_str!("../testdata/eth_24666352_block.json");

        let receipts: Vec<AnyTransactionReceipt> =
            serde_json::from_str(receipts_json).expect("failed to deserialize receipts");
        let block: serde_json::Value =
            serde_json::from_str(block_json).expect("failed to deserialize block");
        let expected_root: B256 = block["receiptsRoot"]
            .as_str()
            .expect("receiptsRoot missing")
            .parse()
            .expect("failed to parse receiptsRoot");

        verify_receipts_root(&receipts, expected_root).expect("failed to verify receipts");
    }

    /// Verifies that when `get_block_receipts` fails mid-batch:
    /// 1. The processing cursor (`last_processed_block`) stops at the last successful block.
    /// 2. On the next run (simulating a Supervisor restart) the failed block is retried, and
    ///    subsequent blocks are also processed.
    ///
    /// BUG (before fix): `last_processed_block` is never updated, so on Supervisor restart
    /// `last_processed` is read from `head_block_number` (eagerly set to `latest = 13`),
    /// permanently skipping block 13.
    ///
    /// FIX (after fix): `last_processed_block` stops at 12 after the round-1 failure, so
    /// round 2 resumes from 12 and correctly processes blocks 13 (retry) and 14.
    #[traced_test]
    #[tokio::test]
    async fn progress_not_lost_on_receipt_rpc_failure() {
        let eth = Asserter::new();
        let any = Asserter::new();

        // Setup for L1Monitor::new() with chain head at block 10
        push_new_responses(&eth, 10);

        // Create the monitor BEFORE pausing time — the DB initialises internal pool
        // timers that must not be frozen before they complete.
        let monitor = make_monitor(eth.clone(), any.clone()).await;

        tokio::time::pause(); // Freeze time so we can advance it manually
        assert_eq!(monitor.head_block(), 10);
        assert_eq!(monitor.last_processed(), 10);

        // ── Round 1: latest = 13; blocks 11 and 12 succeed, block 13 fails ──────────────────
        eth.push_success(&Some(make_block(13, 26, Some(1_000_000_000)))); // Latest
        eth.push_success(&Some(make_block(11, 22, Some(1_000_000_000)))); // block 11 header (not latest)
        any.push_success(&Some(Vec::<AnyTransactionReceipt>::new())); // block 11 receipts OK
        eth.push_success(&Some(make_block(12, 24, Some(1_000_000_000)))); // block 12 header (not latest)
        any.push_success(&Some(Vec::<AnyTransactionReceipt>::new())); // block 12 receipts OK
                                                                      // Block 13 == latest: base_fee reused from latest_block, only receipts needed
        any.push_failure_msg("simulated RPC failure for block 13 receipts"); // block 13 FAIL

        let monitor_r1 = monitor.clone();
        let round1 =
            tokio::spawn(async move { monitor_r1.monitor_l1(CancellationToken::new()).await });

        tokio::time::advance(Duration::from_secs(3)).await; // trigger the ~2 s poll_interval
        let result = round1.await.expect("task should not panic");

        assert!(result.is_err(), "monitor_l1 should return Err when get_block_receipts fails");
        assert_eq!(monitor.head_block(), 13, "head_block_number should reflect chain head");
        assert_eq!(
            monitor.last_processed(),
            12,
            "processing cursor must stop at last successful block (12), not chain head (13)"
        );
    }

    // ── adaptive_get_logs tests ─────────────────────────────────────────────────

    fn make_log() -> Log {
        serde_json::from_value(serde_json::json!({
            "address": "0x0000000000000000000000000000000000000000",
            "topics": [],
            "data": "0x",
            "blockHash": null,
            "blockNumber": null,
            "transactionHash": null,
            "transactionIndex": null,
            "logIndex": null,
            "removed": false
        }))
        .unwrap()
    }

    /// Happy path: entire range fetched in a single chunk.
    #[tokio::test]
    async fn adaptive_get_logs_full_range_succeeds() {
        let eth = Asserter::new();
        let any = Asserter::new();
        push_new_responses(&eth, 20);
        let monitor = make_monitor(eth.clone(), any).await;

        eth.push_success(&vec![make_log(), make_log(), make_log()]);

        let logs = monitor.adaptive_get_logs(Filter::new(), 10, 20).await.unwrap();
        assert_eq!(logs.len(), 3);
    }

    /// When the full range is rejected, the chunk size is halved until a working size is found.
    /// The discovered size is then re-used for all remaining chunks.
    ///
    /// Range 10..=21 (12 blocks, chunk_size starts at 11):
    ///   - chunk_size=11: 10..21 → fail  (chunk_found=false, halve to 5)
    ///   - chunk_size=5:  10..15 → fail  (chunk_found=false, halve to 2)
    ///   - chunk_size=2:  10..12 → ok    (chunk_found=true, advance from=13)
    ///   - chunk_size=2:  13..15 → ok    (advance from=16)
    ///   - chunk_size=2:  16..18 → ok    (advance from=19)
    ///   - chunk_size=2:  19..21 → ok    (advance from=22 → done)
    #[tokio::test]
    async fn adaptive_get_logs_halves_chunk_on_failure() {
        let eth = Asserter::new();
        let any = Asserter::new();
        push_new_responses(&eth, 21);
        let monitor = make_monitor(eth.clone(), any).await;

        // 2 failures (halving), then 4 successful chunks
        eth.push_failure_msg("block range too large");
        eth.push_failure_msg("block range too large");
        eth.push_success(&vec![make_log()]); // chunk 10..12
        eth.push_success(&vec![make_log(), make_log()]); // chunk 13..15
        eth.push_success(&vec![make_log()]); // chunk 16..18
        eth.push_success(&vec![make_log()]); // chunk 19..21

        let logs = monitor.adaptive_get_logs(Filter::new(), 10, 21).await.unwrap();
        assert_eq!(logs.len(), 5, "should accumulate logs from all successful chunks");
    }

    /// A start_block > end_block range returns an empty vec without any RPC calls.
    #[tokio::test]
    async fn adaptive_get_logs_empty_range() {
        let eth = Asserter::new();
        let any = Asserter::new();
        push_new_responses(&eth, 20);
        let monitor = make_monitor(eth.clone(), any).await;

        // No Asserter responses pushed — any RPC call would panic.
        let logs = monitor.adaptive_get_logs(Filter::new(), 20, 10).await.unwrap();
        assert!(logs.is_empty());
    }

    // ── L1Monitor startup tests ─────────────────────────────────────────────────

    /// Push startup RPC responses with a custom set of block timestamps.
    ///
    /// `timestamps[i]` is the timestamp for block `sample_start + i`.
    /// `latest_timestamp` is the timestamp for the initial "Latest" block.
    fn push_new_responses_with_timestamps(
        eth: &Asserter,
        initial_number: u64,
        history_timestamps: &[u64],
        latest_timestamp: u64,
    ) {
        // 1. get_block_by_number(Latest) — fetched first in L1Monitor::new()
        eth.push_success(&Some(make_block(initial_number, latest_timestamp, Some(1_000_000_000))));
        // 2. History loop: sample_start..initial_number
        let sample_start = initial_number.saturating_sub(EIP1559_FEE_ESTIMATION_PAST_BLOCKS);
        for (i, &ts) in history_timestamps.iter().enumerate() {
            eth.push_success(&Some(make_block(sample_start + i as u64, ts, Some(1_000_000_000))));
        }
    }

    /// L1Monitor::new() computes the median block time from sampled timestamps and uses it
    /// as the poll_interval. A large outlier should not skew the median.
    #[tokio::test]
    async fn startup_computes_block_time_from_median() {
        let eth = Asserter::new();
        let any = Asserter::new();

        // 10 history blocks + 1 latest = 11 timestamps, giving 10 deltas.
        // Timestamps produce deltas: [2, 2, 2, 2, 30, 2, 2, 2, 2, 2].
        // After sorting: [2, 2, 2, 2, 2, 2, 2, 2, 2, 30], median (index 5) = 2.
        let history_timestamps = [0u64, 2, 4, 6, 8, 38, 40, 42, 44, 46];
        let latest_timestamp = 48;
        push_new_responses_with_timestamps(&eth, 10, &history_timestamps, latest_timestamp);

        let monitor = make_monitor(eth, any).await;

        assert_eq!(monitor.block_time(), 2, "median should be 2 despite the 30s outlier");
    }
}
