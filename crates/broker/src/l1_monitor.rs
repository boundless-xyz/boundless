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
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    time::Duration,
};

use crate::{
    chain_monitor::{ChainHead, ChainMonitorApi},
    db::DbObj,
    errors::CodedError,
    impl_coded_debug,
    market_monitor::{
        get_block_times, process_log, process_order_submitted, process_request_fulfilled,
        process_request_locked, MarketEvent,
    },
    task::{RetryRes, RetryTask, SupervisorErr},
    OrderRequest, OrderStateChange,
};
use alloy::{
    eips::{BlockId, BlockNumberOrTag},
    network::Ethereum,
    primitives::Address,
    providers::Provider,
    rpc::types::TransactionReceipt,
};
use anyhow::{Context, Result};
use async_trait::async_trait;
use thiserror::Error;
use tokio::sync::{broadcast, mpsc};
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
pub(crate) struct L1Monitor<P> {
    db: DbObj,
    provider: Arc<P>,

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
}

impl<P> L1Monitor<P>
where
    P: Provider<Ethereum> + Send + Sync + 'static,
{
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn new(
        db: DbObj,
        provider: Arc<P>,
        market_addr: Address,
        prover_addr: Address,
        lookback_blocks: u64,
        chain_id: u64,
        new_order_tx: mpsc::Sender<Box<OrderRequest>>,
        order_state_tx: broadcast::Sender<OrderStateChange>,
    ) -> Result<Self> {
        let initial_block = provider
            .get_block_by_number(BlockNumberOrTag::Latest)
            .await
            .context("Failed to get initial block")?
            .context("No block returned for initial fetch")?;

        let initial_number = initial_block.header.number;
        let initial_timestamp = initial_block.header.timestamp;

        let block_time = get_block_times(initial_number, provider.clone()).await?;
        let poll_interval = if block_time > 0 {
            Duration::from_secs(block_time).mul_f32(0.6)
        } else {
            Duration::from_secs(2)
        };

        tracing::info!("L1Monitor initialized at block {initial_number}, block_time={block_time}s, poll_interval={poll_interval:?}");

        Ok(Self {
            db,
            provider,
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
        })
    }

    /// Returns the sampled block time (in seconds) from construction.
    pub(crate) fn block_time(&self) -> u64 {
        self.block_time
    }

    async fn find_open_orders(&self) -> Result<(), L1MonitorErr> {
        // TODO: implement — query historical RequestSubmitted events and replay open orders.
        tracing::info!("find_open_orders stubbed (lookback_blocks={})", self.lookback_blocks,);
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
                        let block_id = BlockId::Number(block_num.into());
                        let receipts = match self
                            .provider
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
        receipts: Vec<TransactionReceipt>,
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
impl<P: Provider<Ethereum> + Send + Sync + 'static> ChainMonitorApi for L1Monitor<P> {
    async fn current_chain_head(&self) -> Result<ChainHead> {
        Ok(ChainHead {
            block_number: self.head_block_number.load(Ordering::Relaxed),
            block_timestamp: self.head_block_timestamp.load(Ordering::Relaxed),
        })
    }

    async fn current_gas_price(&self) -> Result<u128> {
        // TODO: implement gas estimation
        Ok(0)
    }
}

impl<P> RetryTask for L1Monitor<P>
where
    P: Provider<Ethereum> + Send + Sync + Clone + 'static,
{
    type Error = L1MonitorErr;

    fn spawn(&self, cancel_token: CancellationToken) -> RetryRes<Self::Error> {
        let self_clone = self.clone();

        Box::pin(async move {
            tracing::info!("Starting L1Monitor");

            self_clone.find_open_orders().await.map_err(SupervisorErr::Recover)?;

            self_clone.monitor_l1(cancel_token).await.map_err(SupervisorErr::Recover)?;

            Ok(())
        })
    }
}
