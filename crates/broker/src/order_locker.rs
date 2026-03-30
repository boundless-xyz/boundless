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

//! Per-chain OrderLocker that validates, locks, and proves orders dispatched by the
//! [`crate::order_committer::OrderCommitter`].
//!
//! Each chain pipeline has its own OrderLocker instance. Orders are processed immediately
//! on receipt: validated against chain state (expired? locked by others? fulfilled?),
//! checked for gas affordability, then locked on-chain (for `LockAndFulfill`) or inserted
//! directly into the DB (for `FulfillAfterLockExpire`/`FulfillWithoutLocking`).
//!
//! If the pre-lock gas check fails due to a temporary gas spike (`PreLockCheckRetry`),
//! the order is placed in a retry Vec and retried on the next block tick. All other
//! outcomes send a [`CommitmentComplete`] back to the OrderCommitter:
//! - [`CommitmentOutcome::Skipped`] — validation or lock failed, capacity freed immediately
//!
//! Successful commitment does NOT send a completion — the capacity slot stays held until
//! the proving pipeline finishes (ProvingCompleted or ProvingFailed from downstream).

use crate::{
    chain_monitor::{ChainHead, ChainMonitorObj},
    config::ConfigLock,
    db::DbObj,
    errors::CodedError,
    impl_coded_debug, now_timestamp,
    order_committer::{CommitmentComplete, CommitmentOutcome},
    task::{RetryRes, RetryTask, SupervisorErr},
    utils, Erc1271GasCache, FulfillmentType, OrderRequest,
};
use alloy::{
    network::Ethereum,
    primitives::{
        utils::{format_ether, parse_units},
        Address, U256,
    },
    providers::{Provider, WalletProvider},
};
use anyhow::{Context, Result};
use boundless_market::{
    contracts::{
        boundless_market::{BoundlessMarketService, MarketError},
        IBoundlessMarket::IBoundlessMarketErrors,
        RequestStatus, TxnErr,
    },
    dynamic_gas_filler::PriorityMode,
    selector::SupportedSelectors,
};
use std::{collections::HashMap, sync::Arc, time::Duration};
use thiserror::Error;
use tokio::sync::{mpsc, Mutex};
use tokio_util::sync::CancellationToken;

fn estimate_proving_time_no_load(
    order_cycles: Option<u64>,
    config: &OrderLockerConfig,
) -> Option<u64> {
    let cycles = order_cycles?;
    let peak_prove_khz = config.peak_prove_khz?;
    let total = cycles + config.additional_proof_cycles;
    Some(total.div_ceil(1_000).div_ceil(peak_prove_khz))
}

/// Metadata attached to each order for telemetry when recording commitment decisions.
pub(crate) struct OrderCommitmentMeta {
    pub(crate) estimated_proving_time_secs: Option<u64>,
    pub(crate) estimated_proving_time_no_load_secs: Option<u64>,
    pub(crate) concurrent_proving_jobs: u32,
}

struct CapacityResult {
    orders: Vec<Arc<OrderRequest>>,
    meta: HashMap<String, OrderCommitmentMeta>,
}

#[derive(Error)]
pub enum OrderLockerErr {
    #[error("{code} Failed to lock order: {0}", code = self.code())]
    LockTxFailed(String),

    #[error("{code} Failed to confirm lock tx: {0}", code = self.code())]
    LockTxNotConfirmed(String),

    #[error("{code} Insufficient balance for lock", code = self.code())]
    InsufficientBalance,

    #[error("{code} Order already locked", code = self.code())]
    AlreadyLocked,

    #[error("{code} RPC error: {0:?}", code = self.code())]
    RpcErr(anyhow::Error),

    #[error("{code} Unexpected error: {0:?}", code = self.code())]
    UnexpectedError(#[from] anyhow::Error),

    /// Pre-lock gas check failed (e.g. gas spike). Caller may retry next block.
    #[error("{code} Pre-lock gas check failed (retry later): {0}", code = self.code())]
    PreLockCheckRetry(String),
}

impl_coded_debug!(OrderLockerErr);

impl CodedError for OrderLockerErr {
    fn code(&self) -> &str {
        match self {
            OrderLockerErr::LockTxNotConfirmed(_) => "[B-OM-006]",
            OrderLockerErr::LockTxFailed(_) => "[B-OM-007]",
            OrderLockerErr::AlreadyLocked => "[B-OM-009]",
            OrderLockerErr::InsufficientBalance => "[B-OM-010]",
            OrderLockerErr::RpcErr(_) => "[B-OM-011]",
            OrderLockerErr::PreLockCheckRetry(_) => "[B-OM-012]",
            OrderLockerErr::UnexpectedError(_) => "[B-OM-500]",
        }
    }
}

#[derive(Default)]
pub(crate) struct OrderLockerConfig {
    pub(crate) min_deadline: u64,
    pub(crate) peak_prove_khz: Option<u64>,
    pub(crate) max_concurrent_proofs: Option<u32>,
    pub(crate) additional_proof_cycles: u64,
}

#[derive(Clone)]
pub struct RpcRetryConfig {
    pub retry_count: u64,
    pub retry_sleep_ms: u64,
}

/// Per-chain order locker that validates dispatched orders, locks them on-chain (for
/// LockAndFulfill), and inserts them into the per-chain DB for the proving pipeline.
///
/// Orders are processed immediately on receipt from the OrderCommitter. Gas-spike retries
/// are held in a local Vec and retried on the next block tick.
#[derive(Clone)]
pub struct OrderLocker<P> {
    db: DbObj,
    chain_monitor: ChainMonitorObj,
    block_time: u64,
    config: ConfigLock,
    market: BoundlessMarketService<Arc<P>>,
    provider: Arc<P>,
    prover_addr: Address,
    priced_order_rx: Arc<Mutex<mpsc::Receiver<Box<OrderRequest>>>>,
    supported_selectors: SupportedSelectors,
    erc1271_gas_cache: Erc1271GasCache,
    rpc_retry_config: RpcRetryConfig,
    gas_priority_mode: Arc<tokio::sync::RwLock<PriorityMode>>,
    gas_estimation_priority_mode: Arc<tokio::sync::RwLock<PriorityMode>>,
    listen_only: bool,
    chain_id: u64,
    proving_completion_tx: mpsc::Sender<CommitmentComplete>,
}

impl<P> OrderLocker<P>
where
    P: Provider<Ethereum> + WalletProvider + 'static,
{
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        db: DbObj,
        provider: Arc<P>,
        chain_monitor: ChainMonitorObj,
        config: ConfigLock,
        block_time: u64,
        prover_addr: Address,
        market_addr: Address,
        priced_orders_rx: mpsc::Receiver<Box<OrderRequest>>,
        collateral_token_decimals: u8,
        rpc_retry_config: RpcRetryConfig,
        gas_priority_mode: Arc<tokio::sync::RwLock<PriorityMode>>,
        gas_estimation_priority_mode: Arc<tokio::sync::RwLock<PriorityMode>>,
        erc1271_gas_cache: Erc1271GasCache,
        listen_only: bool,
        chain_id: u64,
        proving_completion_tx: mpsc::Sender<CommitmentComplete>,
    ) -> Result<Self> {
        let txn_timeout_opt = {
            let config = config.lock_all().context("Failed to read config")?;
            config.batcher.txn_timeout
        };

        let mut market = BoundlessMarketService::new_for_broker(
            market_addr,
            provider.clone(),
            provider.default_signer_address(),
        );
        market = market.with_timeout(Duration::from_secs(txn_timeout_opt));
        {
            let config = config.lock_all()?;

            market = market.with_collateral_balance_alert(
                &config
                    .market
                    .collateral_balance_warn_threshold
                    .as_ref()
                    .map(|s| parse_units(s, collateral_token_decimals).unwrap().into()),
                &config
                    .market
                    .collateral_balance_error_threshold
                    .as_ref()
                    .map(|s| parse_units(s, collateral_token_decimals).unwrap().into()),
            );
        }
        let monitor = Self {
            db,
            chain_monitor,
            block_time,
            config,
            market,
            provider,
            prover_addr,
            priced_order_rx: Arc::new(Mutex::new(priced_orders_rx)),
            supported_selectors: SupportedSelectors::default(),
            erc1271_gas_cache,
            rpc_retry_config,
            gas_priority_mode,
            gas_estimation_priority_mode,
            listen_only,
            chain_id,
            proving_completion_tx,
        };
        Ok(monitor)
    }

    /// Sends a capacity release signal back to the OrderCommitter. Only called for
    /// `Skipped` outcomes — successful commitment does NOT free capacity (the proving
    /// pipeline holds the slot until ProvingCompleted/ProvingFailed).
    fn send_completion(&self, order: &OrderRequest, outcome: CommitmentOutcome) {
        let _ = self.proving_completion_tx.try_send(CommitmentComplete {
            order_id: order.id(),
            chain_id: self.chain_id,
            outcome,
        });
    }

    async fn lock_order(&self, order: &OrderRequest) -> Result<U256, OrderLockerErr> {
        let request_id = order.request.id;

        let order_status = self
            .market
            .get_status(request_id, Some(order.request.expires_at()))
            .await
            .context("Failed to get request status")
            .map_err(OrderLockerErr::RpcErr)?;
        if order_status != RequestStatus::Unknown {
            tracing::info!("Request {:x} not open: {order_status:?}, skipping", request_id);
            // TODO: fetch some chain data to find out who / and for how much the order
            // was locked in at
            return Err(OrderLockerErr::AlreadyLocked);
        }

        let is_locked = self
            .db
            .is_request_locked(U256::from(order.request.id))
            .await
            .context("Failed to check if request is locked")?;
        if is_locked {
            tracing::warn!("Order {} already locked: {order_status:?}, skipping", order.id());
            return Err(OrderLockerErr::AlreadyLocked);
        }

        let collateral_balance = self
            .market
            .balance_of_collateral(self.prover_addr)
            .await
            .context("Failed to get collateral balance")?;
        if collateral_balance < order.request.offer.lockCollateral {
            tracing::warn!("No longer have enough collateral deposited to market to lock order {}, skipping [need: {} ZKC, have: {} ZKC]", order.id(), format_ether(order.request.offer.lockCollateral), format_ether(collateral_balance));
            return Err(OrderLockerErr::InsufficientBalance);
        }

        tracing::info!(
            "Locking request: 0x{:x} for stake: {}",
            request_id,
            order.request.offer.lockCollateral
        );

        // Pre-lock gas profitability check: compare gas cost against the order's current
        // reward on the pricing ramp. Retries next block on failure (e.g. gas spike).
        if let Some(gas_estimate) = order.gas_estimate {
            let gas_price = self
                .chain_monitor
                .current_gas_price()
                .await
                .map_err(|e| OrderLockerErr::PreLockCheckRetry(format!("gas price: {e:#}")))?;
            let gas_cost = U256::from(gas_price) * U256::from(gas_estimate);
            let reward = order
                .request
                .offer
                .price_at(now_timestamp())
                .map_err(|e| OrderLockerErr::PreLockCheckRetry(e.to_string()))?;
            if gas_cost > reward {
                let msg = format!(
                    "gas cost {} exceeds reward {}",
                    format_ether(gas_cost),
                    format_ether(reward)
                );
                tracing::warn!("Pre-lock check failed for order {}: {msg}", order.id());
                return Err(OrderLockerErr::PreLockCheckRetry(msg));
            }
        }

        let lock_block =
            self.market.lock_request(&order.request, order.client_sig.clone()).await.map_err(
                |e| -> OrderLockerErr {
                    match e {
                        MarketError::TxnError(txn_err) => match txn_err {
                            TxnErr::BoundlessMarketErr(
                                IBoundlessMarketErrors::RequestIsLocked(_),
                            ) => OrderLockerErr::AlreadyLocked,
                            _ => OrderLockerErr::LockTxFailed(txn_err.to_string()),
                        },
                        MarketError::RequestAlreadyLocked(_e) => OrderLockerErr::AlreadyLocked,
                        MarketError::TxnConfirmationError(e) => {
                            OrderLockerErr::LockTxNotConfirmed(e.to_string())
                        }
                        MarketError::LockRevert(e) => {
                            // Note: lock revert could be for any number of reasons;
                            // 1/ someone may have locked in the block before us,
                            // 2/ the lock may have expired,
                            // 3/ the request may have been fulfilled,
                            // 4/ the requestor may have withdrawn their funds
                            // Currently we don't have a way to determine the cause of the revert.
                            OrderLockerErr::LockTxFailed(format!("Tx hash 0x{e:x}"))
                        }
                        MarketError::Error(e) => {
                            // Insufficient balance error is thrown both when the requestor has insufficient balance,
                            // Requestor having insufficient balance can happen and is out of our control. The prover
                            // having insufficient balance is unexpected as we should have checked for that before
                            // committing to locking the order.
                            let prover_addr_str =
                                self.prover_addr.to_string().to_lowercase().replace("0x", "");
                            if e.to_string().contains("InsufficientBalance") {
                                if e.to_string().to_lowercase().contains(&prover_addr_str) {
                                    OrderLockerErr::InsufficientBalance
                                } else {
                                    OrderLockerErr::LockTxFailed(format!(
                                        "Requestor has insufficient balance at lock time: {e}"
                                    ))
                                }
                            } else if e.to_string().contains("RequestIsLocked") {
                                OrderLockerErr::AlreadyLocked
                            } else {
                                OrderLockerErr::UnexpectedError(e)
                            }
                        }
                        _ => {
                            if e.to_string().contains("RequestIsLocked") {
                                OrderLockerErr::AlreadyLocked
                            } else {
                                OrderLockerErr::UnexpectedError(e.into())
                            }
                        }
                    }
                },
            )?;

        // Fetch the block to retrieve the lock timestamp. This has been observed to return
        // inconsistent state between the receipt being available but the block not yet.
        let lock_timestamp = crate::futures_retry::retry(
            self.rpc_retry_config.retry_count,
            self.rpc_retry_config.retry_sleep_ms,
            || async {
                Ok(self
                    .provider
                    .get_block_by_number(lock_block.into())
                    .await
                    .with_context(|| format!("failed to get block {lock_block}"))?
                    .with_context(|| format!("failed to get block {lock_block}: block not found"))?
                    .header
                    .timestamp)
            },
            "get_block_by_number",
        )
        .await
        .map_err(OrderLockerErr::UnexpectedError)?;

        let lock_price = order
            .request
            .offer
            .price_at(lock_timestamp)
            .context("Failed to calculate lock price")?;

        Ok(lock_price)
    }

    async fn skip_order(
        &self,
        order: &OrderRequest,
        skip_commit_code: &str,
        skip_commit_reason: &str,
        config: &OrderLockerConfig,
        estimated_proving_time_secs: Option<u64>,
    ) {
        let concurrent_proving_jobs = match self.db.get_committed_orders().await {
            Ok(committed_orders) => committed_orders.len().try_into().unwrap_or(u32::MAX),
            Err(err) => {
                tracing::error!(
                    "Failed to load committed orders for skipped order telemetry {} - {err:?}",
                    order.id()
                );
                0
            }
        };

        if let Err(e) = self.db.insert_skipped_request(order).await {
            tracing::error!(
                "Failed to skip order ({}): {} - {e:?}",
                skip_commit_reason,
                order.id()
            );
        }

        let no_load = estimate_proving_time_no_load(order.total_cycles, config);
        let monitor_wait =
            order.priced_at_timestamp.map(|t| now_timestamp().saturating_sub(t) * 1000);
        let pending = 0u32;
        let meta = OrderCommitmentMeta {
            estimated_proving_time_secs,
            estimated_proving_time_no_load_secs: no_load,
            concurrent_proving_jobs,
        };

        crate::telemetry::telemetry(self.chain_id).record_order_commitment(
            &order.id(),
            false,
            Some(&meta),
            monitor_wait,
            config,
            pending,
            Some(skip_commit_code),
            Some(skip_commit_reason),
            None,
        );

        self.send_completion(order, CommitmentOutcome::Skipped);
    }

    /// Filters a list of orders against chain state, returning only those still valid.
    /// Invalid orders are skipped with telemetry and a CommitmentOutcome::Skipped signal.
    async fn validate_orders(
        &self,
        orders: Vec<Arc<OrderRequest>>,
        current_block_timestamp: u64,
        min_deadline: u64,
        config: &OrderLockerConfig,
    ) -> Result<Vec<Arc<OrderRequest>>> {
        let mut candidate_orders: Vec<Arc<OrderRequest>> = Vec::new();

        fn is_within_deadline(
            order: &OrderRequest,
            current_block_timestamp: u64,
            min_deadline: u64,
        ) -> bool {
            let expiration = order.expiry();
            if expiration < current_block_timestamp {
                tracing::info!("Request {:x} has now expired. Skipping.", order.request.id);
                false
            } else if expiration.saturating_sub(now_timestamp()) < min_deadline {
                tracing::info!("Request {:x} deadline at {} is less than the minimum deadline {} seconds required to prove an order. Skipping.", order.request.id, expiration, min_deadline);
                false
            } else {
                true
            }
        }

        for order in orders {
            match order.fulfillment_type {
                FulfillmentType::FulfillAfterLockExpire
                | FulfillmentType::FulfillWithoutLocking => {
                    let is_fulfilled = self
                        .db
                        .is_request_fulfilled(U256::from(order.request.id))
                        .await
                        .context("Failed to check if request is fulfilled")?;
                    if is_fulfilled {
                        tracing::info!(
                            "Request 0x{:x} fulfilled by another prover. Skipping.",
                            order.request.id
                        );
                        self.skip_order(
                            &order,
                            "[B-OM-020]",
                            "Order fulfilled by another prover",
                            config,
                            None,
                        )
                        .await;
                    } else if !is_within_deadline(&order, current_block_timestamp, min_deadline) {
                        self.skip_order(&order, "[B-OM-021]", "Order expired", config, None).await;
                    } else if self.market.is_fulfilled(order.request.id).await? {
                        tracing::debug!(
                            "Request 0x{:x} already fulfilled by another prover. Skipping.",
                            order.request.id
                        );
                        self.skip_order(
                            &order,
                            "[B-OM-020]",
                            "Order fulfilled by another prover",
                            config,
                            None,
                        )
                        .await;
                    } else {
                        candidate_orders.push(order);
                    }
                }
                FulfillmentType::LockAndFulfill => {
                    let is_lock_expired = order.request.lock_expires_at() < current_block_timestamp;
                    if is_lock_expired {
                        tracing::info!(
                            "Request {:x} lock has expired. Skipping.",
                            order.request.id
                        );
                        self.skip_order(
                            &order,
                            "[B-OM-022]",
                            "Lock expired before we locked",
                            config,
                            None,
                        )
                        .await;
                    } else if let Some((locker, _)) =
                        self.db.get_request_locked(U256::from(order.request.id)).await?
                    {
                        let our_address =
                            self.provider.default_signer_address().to_string().to_lowercase();
                        let locker_address = locker.to_lowercase();
                        let our_normalized = our_address.trim_start_matches("0x");
                        let locker_normalized = locker_address.trim_start_matches("0x");

                        if locker_normalized != our_normalized {
                            tracing::info!(
                                "Request 0x{:x} already locked by another prover ({}). Skipping.",
                                order.request.id,
                                locker_address
                            );
                            self.skip_order(
                                &order,
                                "[B-OM-023]",
                                "Locked by another prover",
                                config,
                                None,
                            )
                            .await;
                        } else {
                            tracing::info!(
                                "Request 0x{:x} already locked by us. Proceeding to prove.",
                                order.request.id
                            );
                            candidate_orders.push(order);
                        }
                    } else if !is_within_deadline(&order, current_block_timestamp, min_deadline) {
                        self.skip_order(
                            &order,
                            "[B-OM-024]",
                            "Insufficient deadline",
                            config,
                            None,
                        )
                        .await;
                    } else {
                        candidate_orders.push(order);
                    }
                }
            }
        }

        Ok(candidate_orders)
    }

    /// Attempts to lock and/or prove each order. Returns orders that need retry
    /// (PreLockCheckRetry due to gas spikes).
    async fn lock_and_prove_orders(
        &self,
        capacity_result: &CapacityResult,
        locker_config: &OrderLockerConfig,
        retry_commitment_queue: u32,
    ) -> Vec<Arc<OrderRequest>> {
        let mut retry_orders: Vec<Arc<OrderRequest>> = Vec::new();

        for order in &capacity_result.orders {
            let meta = capacity_result.meta.get(&order.id());
            let order_id = order.id();
            let monitor_wait =
                order.priced_at_timestamp.map(|t| now_timestamp().saturating_sub(t) * 1000);

            if order.fulfillment_type == FulfillmentType::LockAndFulfill {
                if self.listen_only {
                    tracing::info!("[LISTEN-ONLY] Would lock and prove order: {}", order_id);
                    self.send_completion(order, CommitmentOutcome::Skipped);
                    continue;
                }

                let lock_submitted_at = std::time::Instant::now();
                match self.lock_order(order).await {
                    Ok(lock_price) => {
                        tracing::info!("Locked request: 0x{:x}", order.request.id);
                        crate::telemetry::telemetry(self.chain_id).record_order_commitment(
                            &order_id,
                            true,
                            meta,
                            monitor_wait,
                            locker_config,
                            retry_commitment_queue,
                            None,
                            None,
                            Some(lock_submitted_at),
                        );
                        if let Err(err) = self.db.insert_accepted_request(order, lock_price).await {
                            tracing::error!(
                                "FATAL STAKE AT RISK: {} failed to move from locking -> proving status {}",
                                order_id,
                                err
                            );
                        }
                    }
                    Err(ref err) => {
                        if let OrderLockerErr::PreLockCheckRetry(reason) = err {
                            tracing::warn!(
                                "Pre-lock check failed for {order_id}: {reason}, will retry next block"
                            );
                            retry_orders.push(order.clone());
                        } else {
                            if matches!(err, OrderLockerErr::UnexpectedError(_)) {
                                tracing::error!("Failed to lock order: {order_id} - {err:?}");
                            } else {
                                tracing::warn!("Failed to lock order: {order_id} - {err:?}");
                            }
                            let skip_reason = err.to_string();
                            crate::telemetry::telemetry(self.chain_id).record_order_commitment(
                                &order_id,
                                false,
                                meta,
                                monitor_wait,
                                locker_config,
                                retry_commitment_queue,
                                Some(err.code()),
                                Some(&skip_reason),
                                Some(lock_submitted_at),
                            );
                            if let Err(e) = self.db.insert_skipped_request(order).await {
                                tracing::error!(
                                    "Failed to set DB failure state for order: {order_id} - {e:?}"
                                );
                            }
                            self.send_completion(order, CommitmentOutcome::Skipped);
                        }
                    }
                }
            } else {
                if self.listen_only {
                    tracing::info!(
                        "[LISTEN-ONLY] Would prove order (after lock expire): {}",
                        order_id
                    );
                    self.send_completion(order, CommitmentOutcome::Skipped);
                    continue;
                }
                crate::telemetry::telemetry(self.chain_id).record_order_commitment(
                    &order_id,
                    true,
                    meta,
                    monitor_wait,
                    locker_config,
                    retry_commitment_queue,
                    None,
                    None,
                    None,
                );
                if let Err(err) = self.db.insert_accepted_request(order, U256::ZERO).await {
                    tracing::error!(
                        "Failed to set order status to pending proving: {} - {err:?}",
                        order_id
                    );
                }
            }
        }

        retry_orders
    }

    /// Calculate the gas units needed for an order and the corresponding cost in wei
    async fn calculate_order_gas_cost_wei(
        &self,
        order: &OrderRequest,
        gas_price: u128,
    ) -> Result<U256, OrderLockerErr> {
        // Calculate gas units needed for this order (lock + fulfill)
        let order_gas_units = if order.fulfillment_type == FulfillmentType::LockAndFulfill {
            U256::from(
                utils::estimate_gas_to_lock(
                    &self.config,
                    order,
                    &self.provider,
                    &self.erc1271_gas_cache,
                )
                .await?,
            )
            .saturating_add(U256::from(
                utils::estimate_gas_to_fulfill(
                    &self.config,
                    &self.supported_selectors,
                    &order.request,
                    order.journal_bytes,
                )
                .await?,
            ))
        } else {
            U256::from(
                utils::estimate_gas_to_fulfill(
                    &self.config,
                    &self.supported_selectors,
                    &order.request,
                    order.journal_bytes,
                )
                .await?,
            )
        };

        let order_cost_wei = U256::from(gas_price) * order_gas_units;

        Ok(order_cost_wei)
    }

    async fn apply_gas_limits(
        &self,
        orders: Vec<Arc<OrderRequest>>,
        config: &OrderLockerConfig,
    ) -> Result<CapacityResult> {
        let num_orders = orders.len();

        let gas_price =
            self.chain_monitor.current_gas_price().await.context("Failed to get gas price")?;
        let available_balance_wei = if self.listen_only {
            U256::MAX
        } else {
            self.provider
                .get_balance(self.provider.default_signer_address())
                .await
                .map_err(|err| OrderLockerErr::RpcErr(err.into()))?
        };

        let committed_orders = self.db.get_committed_orders().await?;
        let committed_gas_units =
            futures::future::try_join_all(committed_orders.iter().map(|order| {
                utils::estimate_gas_to_fulfill(
                    &self.config,
                    &self.supported_selectors,
                    &order.request,
                    order.journal_bytes,
                )
            }))
            .await?
            .iter()
            .sum::<u64>();

        let committed_cost_wei = U256::from(gas_price) * U256::from(committed_gas_units);

        if !committed_orders.is_empty() {
            tracing::debug!(
                "Cost for {} committed orders: {} ether",
                committed_orders.len(),
                format_ether(committed_cost_wei),
            );
        }

        if committed_cost_wei > available_balance_wei {
            tracing::error!(
                "Insufficient balance for committed orders. Required: {} ether, Available: {} ether",
                format_ether(committed_cost_wei),
                format_ether(available_balance_wei)
            );
            return Ok(CapacityResult { orders: Vec::new(), meta: HashMap::new() });
        }

        let mut remaining_balance_wei = available_balance_wei - committed_cost_wei;
        let concurrent_jobs = committed_orders.len() as u32;

        let mut final_orders: Vec<Arc<OrderRequest>> = Vec::with_capacity(num_orders);
        let mut meta: HashMap<String, OrderCommitmentMeta> = HashMap::new();

        for order in orders {
            let order_cost_wei = self.calculate_order_gas_cost_wei(&order, gas_price).await?;

            if order_cost_wei > remaining_balance_wei {
                tracing::warn!(
                    "Insufficient balance for order {}. Required: {} ether, Remaining: {} ether",
                    order.id(),
                    format_ether(order_cost_wei),
                    format_ether(remaining_balance_wei)
                );
                self.skip_order(&order, "[B-OM-025]", "Insufficient balance", config, None).await;
                continue;
            }

            let no_load = estimate_proving_time_no_load(order.total_cycles, config);
            meta.insert(
                order.id(),
                OrderCommitmentMeta {
                    estimated_proving_time_secs: no_load,
                    estimated_proving_time_no_load_secs: no_load,
                    concurrent_proving_jobs: concurrent_jobs,
                },
            );
            final_orders.push(order);
            remaining_balance_wei -= order_cost_wei;
        }

        tracing::debug!(
            "After applying gas limits, filtered {} orders to {}",
            num_orders,
            final_orders.len(),
        );

        Ok(CapacityResult { orders: final_orders, meta })
    }

    /// Runs the full pipeline for a set of orders: validate, apply gas limits, lock/prove.
    /// Orders that hit PreLockCheckRetry are appended to `retry_orders` for the next block tick.
    async fn process_orders(
        &self,
        orders: Vec<Arc<OrderRequest>>,
        config: &OrderLockerConfig,
        block_timestamp: u64,
        retry_orders: &mut Vec<Arc<OrderRequest>>,
    ) -> Result<(), OrderLockerErr> {
        let valid_orders =
            self.validate_orders(orders, block_timestamp, config.min_deadline, config).await?;

        if valid_orders.is_empty() {
            return Ok(());
        }

        let gas_result = self.apply_gas_limits(valid_orders, config).await?;

        if !gas_result.orders.is_empty() {
            let retries =
                self.lock_and_prove_orders(&gas_result, config, retry_orders.len() as u32).await;
            retry_orders.extend(retries);
        }

        Ok(())
    }

    pub async fn start_monitor(
        self,
        cancel_token: CancellationToken,
    ) -> Result<(), OrderLockerErr> {
        let mut last_block = 0;
        let mut interval = tokio::time::interval_at(
            tokio::time::Instant::now(),
            tokio::time::Duration::from_secs(self.block_time),
        );
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        let mut new_orders = self.priced_order_rx.lock().await;
        let mut retry_orders: Vec<Arc<OrderRequest>> = Vec::new();

        let mut cached_gas_priority_mode: Option<PriorityMode> = None;
        let mut cached_gas_estimation_priority_mode: Option<PriorityMode> = None;

        loop {
            tokio::select! {
                biased;

                _ = cancel_token.cancelled() => {
                    tracing::debug!("Order locker received cancellation");
                    break;
                }

                Some(order) = new_orders.recv() => {
                    let order_id = order.id();
                    let ChainHead { block_timestamp, .. } =
                        self.chain_monitor.current_chain_head().await?;
                    let config = {
                        let c = self.config.lock_all().context("Failed to read config")?;
                        OrderLockerConfig {
                            min_deadline: c.market.min_deadline,
                            peak_prove_khz: c.market.peak_prove_khz,
                            max_concurrent_proofs: Some(c.market.max_concurrent_proofs),
                            additional_proof_cycles: c.market.additional_proof_cycles,
                        }
                    };

                    tracing::debug!("Order locker received order {order_id}, processing immediately");
                    self.process_orders(
                        vec![Arc::from(order)],
                        &config,
                        block_timestamp,
                        &mut retry_orders,
                    )
                    .await?;
                }

                _ = interval.tick() => {
                    let ChainHead { block_number, block_timestamp } =
                        self.chain_monitor.current_chain_head().await?;
                    if block_number != last_block {
                        last_block = block_number;

                        let (locker_config, config_gas_mode, config_estimation_mode) = {
                            let config = self.config.lock_all().context("Failed to read config")?;
                            let gas_mode = config.market.gas_priority_mode.clone();
                            let estimation_mode =
                                config.market.gas_estimation_priority_mode.clone();
                            let cfg = OrderLockerConfig {
                                min_deadline: config.market.min_deadline,
                                peak_prove_khz: config.market.peak_prove_khz,
                                max_concurrent_proofs: Some(config.market.max_concurrent_proofs),
                                additional_proof_cycles: config.market.additional_proof_cycles,
                            };
                            (cfg, gas_mode, estimation_mode)
                        };

                        if cached_gas_priority_mode.as_ref() != Some(&config_gas_mode) {
                            let mut current_mode = self.gas_priority_mode.write().await;
                            let old_mode = current_mode.clone();
                            *current_mode = config_gas_mode.clone();
                            drop(current_mode);
                            cached_gas_priority_mode = Some(config_gas_mode.clone());
                            tracing::info!(
                                "Gas priority mode changed from {:?} to {:?}",
                                old_mode,
                                config_gas_mode
                            );
                        }

                        if cached_gas_estimation_priority_mode.as_ref()
                            != Some(&config_estimation_mode)
                        {
                            let mut current_mode =
                                self.gas_estimation_priority_mode.write().await;
                            let old_mode = current_mode.clone();
                            *current_mode = config_estimation_mode.clone();
                            drop(current_mode);
                            cached_gas_estimation_priority_mode = Some(config_estimation_mode);
                            tracing::info!(
                                "Gas estimation priority mode changed from {:?} to {:?}",
                                old_mode,
                                cached_gas_estimation_priority_mode
                            );
                        }

                        if !retry_orders.is_empty() {
                            tracing::debug!(
                                "Order locker processing {} retry orders on block {block_number}",
                                retry_orders.len(),
                            );
                            let retries = std::mem::take(&mut retry_orders);
                            if cancel_token.is_cancelled() {
                                tracing::debug!("Order locker cancellation observed before retry processing");
                                break;
                            }
                            self.process_orders(
                                retries,
                                &locker_config,
                                block_timestamp,
                                &mut retry_orders,
                            )
                            .await?;
                        }
                    }
                }
            }
        }
        Ok(())
    }
}

impl<P> RetryTask for OrderLocker<P>
where
    P: Provider<Ethereum> + WalletProvider + 'static + Clone,
{
    type Error = OrderLockerErr;
    fn spawn(&self, cancel_token: CancellationToken) -> RetryRes<Self::Error> {
        let monitor_clone = self.clone();
        Box::pin(async move {
            tracing::info!("Starting order locker");
            monitor_clone.start_monitor(cancel_token).await.map_err(SupervisorErr::Recover)?;
            Ok(())
        })
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::chain_monitor::ChainMonitorService;
    use crate::OrderStatus;
    use crate::{db::SqliteDb, now_timestamp, proving_order_from_request, FulfillmentType};
    use alloy::{
        network::EthereumWallet,
        node_bindings::{Anvil, AnvilInstance},
        primitives::{address, Address, Bytes, U256},
        providers::{
            fillers::{
                BlobGasFiller, ChainIdFiller, FillProvider, GasFiller, JoinFill, NonceFiller,
                WalletFiller,
            },
            ProviderBuilder, RootProvider,
        },
        signers::local::PrivateKeySigner,
    };
    use boundless_market::contracts::{
        Offer, Predicate, ProofRequest, RequestId, RequestInput, RequestInputType, Requirements,
    };
    use boundless_test_utils::{
        guests::{ASSESSOR_GUEST_ID, ASSESSOR_GUEST_PATH},
        market::{deploy_boundless_market, deploy_hit_points},
    };

    use moka::future::Cache;
    use risc0_zkvm::Digest;
    use std::{future::Future, sync::Arc};
    use tokio::task::JoinSet;
    use tracing_test::traced_test;

    type TestProvider = FillProvider<
        JoinFill<
            JoinFill<
                alloy::providers::Identity,
                JoinFill<GasFiller, JoinFill<BlobGasFiller, JoinFill<NonceFiller, ChainIdFiller>>>,
            >,
            WalletFiller<EthereumWallet>,
        >,
        RootProvider,
    >;

    pub struct TestCtx {
        pub monitor: OrderLocker<TestProvider>,
        pub anvil: AnvilInstance,
        pub db: DbObj,
        pub market_address: Address,
        pub config: ConfigLock,
        pub priced_order_tx: mpsc::Sender<Box<OrderRequest>>,
        pub signer: PrivateKeySigner,
        pub market_service: BoundlessMarketService<Arc<TestProvider>>,
        next_order_id: u32,
    }

    impl TestCtx {
        pub async fn create_test_order(
            &mut self,
            fulfillment_type: FulfillmentType,
            bidding_start: u64,
            lock_timeout: u64,
            timeout: u64,
        ) -> Box<OrderRequest> {
            let request_id = self.next_order_id;
            self.next_order_id += 1;

            let request = ProofRequest::new(
                RequestId::new(self.signer.address(), request_id),
                Requirements::new(Predicate::prefix_match(Digest::ZERO, Bytes::default())),
                "http://risczero.com/image",
                RequestInput { inputType: RequestInputType::Inline, data: Default::default() },
                Offer {
                    minPrice: U256::from(1),
                    maxPrice: U256::from(2),
                    rampUpStart: bidding_start,
                    rampUpPeriod: 1,
                    timeout: timeout as u32,
                    lockTimeout: lock_timeout as u32,
                    lockCollateral: U256::from(0),
                },
            );

            let client_sig = request
                .sign_request(&self.signer, self.market_address, self.anvil.chain_id())
                .await
                .unwrap()
                .as_bytes()
                .into();

            let mut order = OrderRequest::new(
                request,
                client_sig,
                fulfillment_type,
                self.market_address,
                self.anvil.chain_id(),
            );
            order.target_timestamp = Some(0);
            Box::new(order)
        }
    }

    pub async fn setup_oc_test_context() -> TestCtx {
        let anvil = Anvil::new().spawn();
        let signer: PrivateKeySigner = anvil.keys()[0].clone().into();
        let provider = Arc::new(
            ProviderBuilder::new()
                .wallet(EthereumWallet::from(signer.clone()))
                .connect(&anvil.endpoint())
                .await
                .unwrap(),
        );

        let hit_points = deploy_hit_points(signer.address(), provider.clone()).await.unwrap();

        let market_address = deploy_boundless_market(
            signer.address(),
            provider.clone(),
            address!("0x0000000000000000000000000000000000000001"),
            hit_points,
            Digest::from(ASSESSOR_GUEST_ID),
            format!("file://{ASSESSOR_GUEST_PATH}"),
            Some(signer.address()),
        )
        .await
        .unwrap();

        let market_service = BoundlessMarketService::new_for_broker(
            market_address,
            provider.clone(),
            provider.default_signer_address(),
        );

        let collateral_token_decimals = market_service.collateral_token_decimals().await.unwrap();
        market_service
            .deposit(parse_units("10.0", collateral_token_decimals).unwrap().into())
            .await
            .unwrap();

        let db: DbObj = Arc::new(SqliteDb::new("sqlite::memory:").await.unwrap());
        let config = ConfigLock::default();

        config.load_write().unwrap().market.min_deadline = 50;
        config.load_write().unwrap().market.lockin_gas_estimate = 200_000;
        config.load_write().unwrap().market.fulfill_gas_estimate = 300_000;
        config.load_write().unwrap().market.groth16_verify_gas_estimate = 50_000;

        let block_time = 2;

        let gas_estimation_priority_mode =
            Arc::new(tokio::sync::RwLock::new(PriorityMode::default()));
        let chain_monitor_service = Arc::new(
            ChainMonitorService::new(provider.clone(), gas_estimation_priority_mode).await.unwrap(),
        );
        tokio::spawn(chain_monitor_service.spawn(Default::default()));
        let chain_monitor: ChainMonitorObj = chain_monitor_service;

        let (priced_order_tx, priced_order_rx) = mpsc::channel(16);

        let gas_priority_mode = Arc::new(tokio::sync::RwLock::new(PriorityMode::Medium));
        let gas_estimation_priority_mode =
            Arc::new(tokio::sync::RwLock::new(PriorityMode::default()));

        let (proving_completion_tx, _proving_completion_rx) = mpsc::channel(100);

        let monitor = OrderLocker::new(
            db.clone(),
            provider.clone(),
            chain_monitor.clone(),
            config.clone(),
            block_time,
            signer.address(),
            market_address,
            priced_order_rx,
            collateral_token_decimals,
            RpcRetryConfig { retry_count: 2, retry_sleep_ms: 500 },
            gas_priority_mode,
            gas_estimation_priority_mode,
            Arc::new(Cache::builder().build()),
            false,
            anvil.chain_id(),
            proving_completion_tx,
        )
        .unwrap();

        TestCtx {
            monitor,
            anvil,
            db,
            market_address,
            config,
            priced_order_tx,
            signer,
            market_service,
            next_order_id: 1,
        }
    }

    async fn run_with_monitor<P, F, T>(monitor: OrderLocker<P>, f: F) -> T
    where
        P: Provider + WalletProvider + Clone + 'static,
        F: Future<Output = T>,
    {
        let mut tasks = JoinSet::new();
        tasks.spawn(async move { monitor.start_monitor(Default::default()).await });

        tokio::select! {
            result = f => result,
            monitor_task_result = tasks.join_next() => {
                panic!("Monitor exited unexpectedly: {:?}", monitor_task_result.unwrap());
            },
        }
    }

    #[tokio::test]
    #[traced_test]
    async fn monitor_processes_order_immediately() {
        let mut ctx = setup_oc_test_context().await;

        let order =
            ctx.create_test_order(FulfillmentType::LockAndFulfill, now_timestamp(), 100, 200).await;
        let order_id = order.id();

        let _request_id =
            ctx.market_service.submit_request(&order.request, &ctx.signer).await.unwrap();

        ctx.priced_order_tx.send(order).await.unwrap();

        run_with_monitor(ctx.monitor, async move {
            for _ in 0..20 {
                let order = ctx.db.get_order(&order_id).await.unwrap();
                if order.is_some() {
                    assert_eq!(order.unwrap().status, OrderStatus::PendingProving);
                    break;
                }
                tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
            }

            let order = ctx.db.get_order(&order_id).await.unwrap().unwrap();
            assert_eq!(order.status, OrderStatus::PendingProving);
        })
        .await;
    }

    #[tokio::test]
    #[traced_test]
    async fn test_filter_expired_orders() {
        let mut ctx = setup_oc_test_context().await;
        let current_timestamp = now_timestamp();

        let expired_order = ctx
            .create_test_order(FulfillmentType::LockAndFulfill, current_timestamp - 100, 50, 50)
            .await;
        let expired_order_id = expired_order.id();

        let result = ctx
            .monitor
            .validate_orders(
                vec![Arc::from(expired_order)],
                current_timestamp,
                0,
                &OrderLockerConfig::default(),
            )
            .await
            .unwrap();

        assert!(result.is_empty());

        let order = ctx.db.get_order(&expired_order_id).await.unwrap().unwrap();
        assert_eq!(order.status, OrderStatus::Skipped);
    }

    #[tokio::test]
    #[traced_test]
    async fn test_filter_insufficient_deadline() {
        let mut ctx = setup_oc_test_context().await;
        let current_timestamp = now_timestamp();

        let order1 =
            ctx.create_test_order(FulfillmentType::LockAndFulfill, current_timestamp, 45, 45).await;
        let order_1_id = order1.id();

        let order2 = ctx
            .create_test_order(FulfillmentType::FulfillAfterLockExpire, current_timestamp, 1, 45)
            .await;
        let order_2_id = order2.id();

        let result = ctx
            .monitor
            .validate_orders(
                vec![Arc::from(order1), Arc::from(order2)],
                current_timestamp,
                100,
                &OrderLockerConfig::default(),
            )
            .await
            .unwrap();

        assert!(result.is_empty());

        let order = ctx.db.get_order(&order_1_id).await.unwrap().unwrap();
        assert_eq!(order.status, OrderStatus::Skipped);

        let order = ctx.db.get_order(&order_2_id).await.unwrap().unwrap();
        assert_eq!(order.status, OrderStatus::Skipped);
    }

    #[tokio::test]
    async fn test_filter_locked_by_others() {
        let mut ctx = setup_oc_test_context().await;
        let current_timestamp = now_timestamp();

        let order = ctx
            .create_test_order(FulfillmentType::LockAndFulfill, current_timestamp, 100, 200)
            .await;
        let order_id = order.id();
        ctx.db
            .set_request_locked(
                U256::from(order.request.id),
                &Address::ZERO.to_string(),
                current_timestamp,
            )
            .await
            .unwrap();

        let result = ctx
            .monitor
            .validate_orders(
                vec![Arc::from(order)],
                current_timestamp,
                current_timestamp + 100,
                &OrderLockerConfig::default(),
            )
            .await
            .unwrap();

        assert!(result.is_empty());

        let order = ctx.db.get_order(&order_id).await.unwrap().unwrap();
        assert_eq!(order.status, OrderStatus::Skipped);
    }

    #[tokio::test]
    #[traced_test]
    async fn test_process_fulfill_after_lock_expire_orders() {
        let mut ctx = setup_oc_test_context().await;
        let current_timestamp = now_timestamp();

        let order = ctx
            .create_test_order(FulfillmentType::FulfillAfterLockExpire, current_timestamp, 100, 200)
            .await;
        let order_id = order.id();

        let capacity_result =
            CapacityResult { orders: vec![Arc::from(order)], meta: HashMap::new() };
        ctx.monitor.lock_and_prove_orders(&capacity_result, &OrderLockerConfig::default(), 0).await;

        let updated_order = ctx.db.get_order(&order_id).await.unwrap().unwrap();
        assert_eq!(updated_order.status, OrderStatus::PendingProving);
    }

    #[tokio::test]
    #[traced_test]
    async fn test_apply_gas_limits_passes_all_orders() {
        let mut ctx = setup_oc_test_context().await;
        let current_timestamp = now_timestamp();

        let mut orders = Vec::new();
        for _ in 1..=5 {
            let order = ctx
                .create_test_order(FulfillmentType::LockAndFulfill, current_timestamp, 100, 200)
                .await;

            let _request_id =
                ctx.market_service.submit_request(&order.request, &ctx.signer).await.unwrap();

            orders.push(Arc::from(order));
        }

        let gas_result = ctx
            .monitor
            .apply_gas_limits(orders.clone(), &OrderLockerConfig::default())
            .await
            .unwrap();
        let retries =
            ctx.monitor.lock_and_prove_orders(&gas_result, &OrderLockerConfig::default(), 0).await;
        assert!(retries.is_empty(), "No orders should need retry");

        let mut processed_count = 0;
        for order in orders {
            if let Some(order) = ctx.db.get_order(&order.id()).await.unwrap() {
                processed_count += 1;
                assert_eq!(order.status, OrderStatus::PendingProving);
            }
        }

        assert_eq!(
            processed_count, 5,
            "Should have processed all 5 orders when gas balance is sufficient"
        );
    }

    #[tokio::test]
    async fn test_gas_estimation_functions() {
        let mut ctx = setup_oc_test_context().await;

        let lock_and_fulfill_order =
            ctx.create_test_order(FulfillmentType::LockAndFulfill, now_timestamp(), 100, 200).await;
        let lock_and_fulfill_id = lock_and_fulfill_order.id();

        let fulfill_only_order = ctx
            .create_test_order(FulfillmentType::FulfillAfterLockExpire, now_timestamp(), 100, 200)
            .await;
        let fulfill_only_id = fulfill_only_order.id();

        let _lock_request_id = ctx
            .market_service
            .submit_request(&lock_and_fulfill_order.request, &ctx.signer)
            .await
            .unwrap();
        let _fulfill_request_id = ctx
            .market_service
            .submit_request(&fulfill_only_order.request, &ctx.signer)
            .await
            .unwrap();

        let orders = vec![Arc::from(lock_and_fulfill_order), Arc::from(fulfill_only_order)];
        let gas_result =
            ctx.monitor.apply_gas_limits(orders, &OrderLockerConfig::default()).await.unwrap();
        let retries =
            ctx.monitor.lock_and_prove_orders(&gas_result, &OrderLockerConfig::default(), 0).await;
        assert!(retries.is_empty());

        let lock_order_result = ctx.db.get_order(&lock_and_fulfill_id).await.unwrap();
        let fulfill_order_result = ctx.db.get_order(&fulfill_only_id).await.unwrap();

        assert!(lock_order_result.is_some());
        assert!(fulfill_order_result.is_some());

        assert_eq!(lock_order_result.unwrap().status, OrderStatus::PendingProving);
        assert_eq!(fulfill_order_result.unwrap().status, OrderStatus::PendingProving);
    }

    #[tokio::test]
    #[traced_test]
    async fn test_insufficient_balance_committed_orders() {
        let mut ctx = setup_oc_test_context().await;

        let balance = ctx.monitor.provider.get_balance(ctx.signer.address()).await.unwrap();
        let gas_price = ctx.monitor.provider.get_gas_price().await.unwrap();
        let gas_remaining: u64 = (balance / U256::from(gas_price)).try_into().unwrap();
        ctx.config.load_write().unwrap().market.fulfill_gas_estimate = gas_remaining / 2;
        ctx.config.load_write().unwrap().market.lockin_gas_estimate = gas_remaining / 3;

        let incoming_order =
            ctx.create_test_order(FulfillmentType::LockAndFulfill, now_timestamp(), 100, 200).await;

        let mut orders = vec![Arc::from(incoming_order)];

        let gas_result = ctx
            .monitor
            .apply_gas_limits(orders.clone(), &OrderLockerConfig::default())
            .await
            .unwrap();
        assert_eq!(gas_result.orders.len(), 1);

        orders.push(Arc::from(
            ctx.create_test_order(FulfillmentType::LockAndFulfill, now_timestamp(), 100, 200).await,
        ));

        let gas_result = ctx
            .monitor
            .apply_gas_limits(orders.clone(), &OrderLockerConfig::default())
            .await
            .unwrap();
        assert_eq!(gas_result.orders.len(), 1);

        for _ in 0..3 {
            let committed_order = ctx
                .create_test_order(FulfillmentType::LockAndFulfill, now_timestamp(), 100, 200)
                .await;

            let mut committed_order_obj =
                proving_order_from_request(&committed_order, Default::default());
            committed_order_obj.status = OrderStatus::Proving;
            committed_order_obj.proving_started_at = Some(now_timestamp());
            ctx.db.add_order(&committed_order_obj).await.unwrap();
        }

        let gas_result =
            ctx.monitor.apply_gas_limits(orders, &OrderLockerConfig::default()).await.unwrap();

        assert!(gas_result.orders.is_empty());
    }

    #[tokio::test]
    #[traced_test]
    async fn test_insufficient_collateral_balance() {
        let mut ctx = setup_oc_test_context().await;
        let current_timestamp = now_timestamp();

        let mut order = ctx
            .create_test_order(FulfillmentType::LockAndFulfill, current_timestamp, 100, 200)
            .await;

        let collateral_token_decimals =
            ctx.market_service.collateral_token_decimals().await.unwrap();
        order.request.offer.lockCollateral =
            parse_units("20.0", collateral_token_decimals).unwrap().into();

        order.client_sig = order
            .request
            .sign_request(&ctx.signer, ctx.market_address, ctx.anvil.chain_id())
            .await
            .unwrap()
            .as_bytes()
            .into();

        let order_id = order.id();

        let _submitted_request_id =
            ctx.market_service.submit_request(&order.request, &ctx.signer).await.unwrap();

        let valid_orders = ctx
            .monitor
            .validate_orders(
                vec![Arc::from(order)],
                current_timestamp,
                50,
                &OrderLockerConfig::default(),
            )
            .await
            .unwrap();
        let capacity_result = CapacityResult { orders: valid_orders, meta: HashMap::new() };
        ctx.monitor.lock_and_prove_orders(&capacity_result, &OrderLockerConfig::default(), 0).await;

        let skipped_order = ctx.db.get_order(&order_id).await.unwrap();
        assert!(skipped_order.is_some(), "Order should be in database");
        assert_eq!(
            skipped_order.unwrap().status,
            OrderStatus::Skipped,
            "Order should be skipped due to insufficient collateral balance"
        );

        assert!(
            logs_contain("No longer have enough collateral deposited to market to lock order"),
            "Expected log message about insufficient collateral balance"
        );
    }

    #[tokio::test]
    #[traced_test]
    async fn pre_lock_check_gas_too_high_retries() {
        let mut ctx = setup_oc_test_context().await;

        let mut order =
            ctx.create_test_order(FulfillmentType::LockAndFulfill, now_timestamp(), 100, 200).await;
        order.gas_estimate = Some(500_000);
        let _ = ctx.market_service.submit_request(&order.request, &ctx.signer).await.unwrap();
        let order_arc = Arc::new(*order);
        let order_id = order_arc.id().clone();

        let capacity_result = CapacityResult { orders: vec![order_arc], meta: HashMap::new() };
        let retries = ctx
            .monitor
            .lock_and_prove_orders(&capacity_result, &OrderLockerConfig::default(), 0)
            .await;

        assert_eq!(retries.len(), 1, "Order should be in retry queue");
        assert_eq!(retries[0].id(), order_id);
        assert!(logs_contain("gas cost"), "Expected log about gas cost exceeding reward");
    }
}
