// Copyright (c) 2025 RISC Zero, Inc.
//
// All rights reserved.

use crate::chain_monitor::ChainHead;
use crate::OrderRequest;
use crate::{
    chain_monitor::ChainMonitorService,
    config::ConfigLock,
    db::DbObj,
    errors::CodedError,
    impl_coded_debug, now_timestamp,
    task::{RetryRes, RetryTask, SupervisorErr},
    utils, FulfillmentType, Order,
};
use alloy::{
    network::Ethereum,
    primitives::{
        utils::{format_ether, parse_ether},
        Address, U256,
    },
    providers::{Provider, WalletProvider},
};
use anyhow::{Context, Result};
use boundless_market::contracts::{
    boundless_market::{BoundlessMarketService, MarketError},
    IBoundlessMarket::IBoundlessMarketErrors,
    RequestStatus, TxnErr,
};
use boundless_market::selector::SupportedSelectors;
use moka::{future::Cache, Expiry};
use std::sync::Arc;
use std::time::{Duration, Instant};
use thiserror::Error;
use tokio::sync::{mpsc, Mutex};

/// Hard limit on the number of orders to concurrently kick off proving work for.
const MAX_PROVING_BATCH_SIZE: u32 = 10;

#[derive(Error)]
pub enum OrderMonitorErr {
    #[error("{code} Failed to lock order: {0}", code = self.code())]
    LockTxFailed(String),

    #[error("{code} Failed to confirm lock tx: {0}", code = self.code())]
    LockTxNotConfirmed(String),

    #[error("{code} Insufficient balance for lock", code = self.code())]
    InsufficientBalance,

    #[error("{code} Order already locked", code = self.code())]
    AlreadyLocked,

    #[error("{code} Unexpected error: {0:?}", code = self.code())]
    UnexpectedError(#[from] anyhow::Error),
}

impl_coded_debug!(OrderMonitorErr);

impl CodedError for OrderMonitorErr {
    fn code(&self) -> &str {
        match self {
            OrderMonitorErr::LockTxNotConfirmed(_) => "[B-OM-006]",
            OrderMonitorErr::LockTxFailed(_) => "[B-OM-007]",
            OrderMonitorErr::AlreadyLocked => "[B-OM-009]",
            OrderMonitorErr::InsufficientBalance => "[B-OM-010]",
            OrderMonitorErr::UnexpectedError(_) => "[B-OM-500]",
        }
    }
}

/// Represents the capacity for proving orders that we have available given our config.
/// Also manages vending out capacity for proving, preventing too many proofs from being
/// kicked off in each iteration.
#[derive(Debug, PartialEq)]
enum Capacity {
    /// There are orders that have been picked for proving but not fulfilled yet.
    /// Number indicates available slots.
    Proving(u32),
    /// There is no concurrent lock limit.
    Unlimited,
}

impl Capacity {
    /// Returns the number of proofs we can kick off in the current iteration. Capped at
    /// [MAX_PROVING_BATCH_SIZE] to limit number of proving tasks spawned at once.
    fn request_capacity(&self, request: u32) -> u32 {
        match self {
            Capacity::Proving(capacity) => {
                if request > *capacity {
                    std::cmp::min(*capacity, MAX_PROVING_BATCH_SIZE)
                } else {
                    std::cmp::min(request, MAX_PROVING_BATCH_SIZE)
                }
            }
            Capacity::Unlimited => std::cmp::min(MAX_PROVING_BATCH_SIZE, request),
        }
    }
}

struct OrderExpiry;

impl<K: std::hash::Hash + Eq, V: std::borrow::Borrow<OrderRequest>> Expiry<K, V> for OrderExpiry {
    fn expire_after_create(&self, _key: &K, value: &V, _now: Instant) -> Option<Duration> {
        let order: &OrderRequest = value.borrow();
        order.expire_timestamp.map(|t| {
            let time_until_expiry = t.saturating_sub(now_timestamp());
            Duration::from_secs(time_until_expiry)
        })
    }
}

#[derive(Clone)]
pub struct OrderMonitor<P> {
    db: DbObj,
    chain_monitor: Arc<ChainMonitorService<P>>,
    block_time: u64,
    config: ConfigLock,
    market: BoundlessMarketService<Arc<P>>,
    provider: Arc<P>,
    prover_addr: Address,
    priced_order_rx: Arc<Mutex<mpsc::Receiver<Box<OrderRequest>>>>,
    lock_and_prove_cache: Arc<Cache<String, Arc<OrderRequest>>>,
    prove_cache: Arc<Cache<String, Arc<OrderRequest>>>,
    supported_selectors: SupportedSelectors,
}

impl<P> OrderMonitor<P>
where
    P: Provider<Ethereum> + WalletProvider,
{
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        db: DbObj,
        provider: Arc<P>,
        chain_monitor: Arc<ChainMonitorService<P>>,
        config: ConfigLock,
        block_time: u64,
        prover_addr: Address,
        market_addr: Address,
        priced_orders_rx: mpsc::Receiver<Box<OrderRequest>>,
    ) -> Result<Self> {
        let txn_timeout_opt = {
            let config = config.lock_all().context("Failed to read config")?;
            config.batcher.txn_timeout
        };

        let mut market = BoundlessMarketService::new(
            market_addr,
            provider.clone(),
            provider.default_signer_address(),
        );
        if let Some(txn_timeout) = txn_timeout_opt {
            market = market.with_timeout(Duration::from_secs(txn_timeout));
        }
        {
            let config = config.lock_all()?;
            market = market.with_stake_balance_alert(
                &config
                    .market
                    .stake_balance_warn_threshold
                    .as_ref()
                    .map(|s| parse_ether(s))
                    .transpose()?,
                &config
                    .market
                    .stake_balance_error_threshold
                    .as_ref()
                    .map(|s| parse_ether(s))
                    .transpose()?,
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
            lock_and_prove_cache: Arc::new(Cache::builder().expire_after(OrderExpiry).build()),
            prove_cache: Arc::new(Cache::builder().expire_after(OrderExpiry).build()),
            supported_selectors: SupportedSelectors::default(),
        };
        Ok(monitor)
    }

    async fn lock_order(&self, order: &OrderRequest) -> Result<U256, OrderMonitorErr> {
        let request_id = order.request.id;

        let order_status = self
            .market
            .get_status(request_id, Some(order.request.expires_at()))
            .await
            .context("Failed to get request status")?;
        if order_status != RequestStatus::Unknown {
            tracing::info!("Request {:x} not open: {order_status:?}, skipping", request_id);
            // TODO: fetch some chain data to find out who / and for how much the order
            // was locked in at
            return Err(OrderMonitorErr::AlreadyLocked);
        }

        let is_locked = self
            .db
            .is_request_locked(U256::from(order.request.id))
            .await
            .context("Failed to check if request is locked")?;
        if is_locked {
            tracing::warn!("Request 0x{:x} already locked: {order_status:?}, skipping", request_id);
            return Err(OrderMonitorErr::AlreadyLocked);
        }

        let conf_priority_gas = {
            let conf = self.config.lock_all().context("Failed to lock config")?;
            conf.market.lockin_priority_gas
        };

        tracing::info!(
            "Locking request: 0x{:x} for stake: {}",
            request_id,
            order.request.offer.lockStake
        );
        let lock_block = self
            .market
            .lock_request(&order.request, &order.client_sig, conf_priority_gas)
            .await
            .map_err(|e| -> OrderMonitorErr {
                match e {
                    MarketError::TxnError(txn_err) => match txn_err {
                        TxnErr::BoundlessMarketErr(IBoundlessMarketErrors::RequestIsLocked(_)) => {
                            OrderMonitorErr::AlreadyLocked
                        }
                        _ => OrderMonitorErr::LockTxFailed(txn_err.to_string()),
                    },
                    MarketError::RequestAlreadyLocked(_e) => OrderMonitorErr::AlreadyLocked,
                    MarketError::TxnConfirmationError(e) => {
                        OrderMonitorErr::LockTxNotConfirmed(e.to_string())
                    }
                    MarketError::LockRevert(e) => {
                        // Note: lock revert could be for any number of reasons;
                        // 1/ someone may have locked in the block before us,
                        // 2/ the lock may have expired,
                        // 3/ the request may have been fulfilled,
                        // 4/ the requestor may have withdrawn their funds
                        // Currently we don't have a way to determine the cause of the revert.
                        OrderMonitorErr::LockTxFailed(format!("Tx hash 0x{:x}", e))
                    }
                    MarketError::Error(e) => {
                        // Insufficient balance error is thrown both when the requestor has insufficient balance,
                        // Requestor having insufficient balance can happen and is out of our control. The prover
                        // having insufficient balance is unexpected as we should have checked for that before
                        // committing to locking the order.
                        let prover_addr_str =
                            self.prover_addr.to_string().to_lowercase().replace("0x", "");
                        if e.to_string().contains("InsufficientBalance") {
                            if e.to_string().contains(&prover_addr_str) {
                                OrderMonitorErr::InsufficientBalance
                            } else {
                                OrderMonitorErr::LockTxFailed(format!(
                                    "Requestor has insufficient balance at lock time: {}",
                                    e
                                ))
                            }
                        } else {
                            OrderMonitorErr::UnexpectedError(e)
                        }
                    }
                    _ => OrderMonitorErr::UnexpectedError(e.into()),
                }
            })?;

        let lock_timestamp = self
            .provider
            .get_block_by_number(lock_block.into())
            .await
            .with_context(|| format!("failed to get block {lock_block}"))?
            .with_context(|| format!("failed to get block {lock_block}: block not found"))?
            .header
            .timestamp;

        let lock_price = order
            .request
            .offer
            .price_at(lock_timestamp)
            .context("Failed to calculate lock price")?;

        Ok(lock_price)
    }

    async fn get_proving_order_capacity(
        &self,
        max_concurrent_proofs: Option<u32>,
    ) -> Result<Capacity, OrderMonitorErr> {
        if max_concurrent_proofs.is_none() {
            return Ok(Capacity::Unlimited);
        };

        let max = max_concurrent_proofs.unwrap();
        let committed_orders = self
            .db
            .get_committed_orders()
            .await
            .map_err(|e| OrderMonitorErr::UnexpectedError(e.into()))?;
        let committed_orders_count: u32 = committed_orders.len().try_into().unwrap();

        self.log_capacity(committed_orders, max).await;

        let available_slots = max.saturating_sub(committed_orders_count);
        Ok(Capacity::Proving(available_slots))
    }

    async fn log_capacity(&self, commited_orders: Vec<Order>, max: u32) {
        let committed_orders_count: u32 = commited_orders.len().try_into().unwrap();
        let request_id_and_status = commited_orders
            .iter()
            .map(|order| {
                (
                    format!("0x{:x}", order.request.id),
                    order.status,
                    order.fulfillment_type,
                    format!(
                        "Lock Expire: {}, Request Expire: {}",
                        order.request.lock_expires_at(),
                        order.request.expires_at()
                    ),
                )
            })
            .collect::<Vec<_>>();

        let capacity_debug_log = format!("Current num committed orders: {committed_orders_count}. Maximum commitment: {max}. Committed orders: {request_id_and_status:?}");
        tracing::info!("{}", capacity_debug_log);
    }

    /// Helper method to skip an order in the database and invalidate the appropriate cache
    async fn skip_order(&self, order: &OrderRequest, reason: &str) {
        if let Err(e) = self.db.insert_skipped_request(order).await {
            tracing::error!("Failed to skip order ({}): {} - {e:?}", reason, order.id());
        }

        match order.fulfillment_type {
            FulfillmentType::LockAndFulfill => {
                self.lock_and_prove_cache.invalidate(&order.id()).await;
            }
            FulfillmentType::FulfillAfterLockExpire | FulfillmentType::FulfillWithoutLocking => {
                self.prove_cache.invalidate(&order.id()).await;
            }
        }
    }

    async fn get_valid_orders(
        &self,
        current_block_timestamp: u64,
        min_deadline: u64,
    ) -> Result<Vec<Arc<OrderRequest>>> {
        let mut candidate_orders: Vec<Arc<OrderRequest>> = Vec::new();

        fn is_within_deadline(
            order: &OrderRequest,
            current_block_timestamp: u64,
            min_deadline: u64,
        ) -> bool {
            if order.request.expires_at() < current_block_timestamp {
                tracing::debug!("Request {:x} has now expired. Skipping.", order.request.id);
                false
            } else if order.request.expires_at().saturating_sub(now_timestamp()) < min_deadline {
                tracing::debug!("Request {:x} deadline at {} is less than the minimum deadline {} seconds required to prove an order. Skipping.", order.request.id, order.request.expires_at(), min_deadline);
                false
            } else {
                true
            }
        }

        for (_, order) in self.prove_cache.iter() {
            let is_fulfilled = self
                .db
                .is_request_fulfilled(U256::from(order.request.id))
                .await
                .context("Failed to check if request is fulfilled")?;
            if is_fulfilled {
                tracing::info!(
                    "Request 0x{:x} was locked by another prover and was fulfilled. Skipping.",
                    order.request.id
                );
                self.skip_order(&order, "was fulfilled by other").await;
            } else if !is_within_deadline(&order, current_block_timestamp, min_deadline) {
                self.skip_order(&order, "expired").await;
            } else {
                tracing::info!("Request 0x{:x} was locked by another prover but expired unfulfilled, setting status to pending proving", order.request.id);
                candidate_orders.push(order);
            }
        }

        for (_, order) in self.lock_and_prove_cache.iter() {
            let is_lock_expired = order.request.lock_expires_at() < current_block_timestamp;
            if is_lock_expired {
                tracing::debug!("Request {:x} was scheduled to be locked by us, but its lock has now expired. Skipping.", order.request.id);
                self.skip_order(&order, "lock expired before we locked").await;
            } else if let Some((locker, _)) =
                self.db.get_request_locked(U256::from(order.request.id)).await?
            {
                let our_address = self.provider.default_signer_address().to_string().to_lowercase();
                let locker_address = locker.to_lowercase();
                // Compare normalized addresses (lowercase without 0x prefix)
                let our_address_normalized = our_address.trim_start_matches("0x");
                let locker_address_normalized = locker_address.trim_start_matches("0x");

                if locker_address_normalized != our_address_normalized {
                    tracing::debug!("Request 0x{:x} was scheduled to be locked by us ({}), but is already locked by another prover ({}). Skipping.", order.request.id, our_address, locker_address);
                    self.skip_order(&order, "locked by another prover").await;
                } else {
                    // Edge case where we locked the order, but due to some reason was not moved to proving state. Should not happen.
                    tracing::debug!("Request 0x{:x} was scheduled to be locked by us, but is already locked by us. Proceeding to prove.", order.request.id);
                    candidate_orders.push(order);
                }
            } else if !is_within_deadline(&order, current_block_timestamp, min_deadline) {
                self.skip_order(&order, "insufficient deadline").await;
            } else {
                candidate_orders.push(order);
            }
        }

        if candidate_orders.is_empty() {
            tracing::trace!(
                "No orders to lock and/or prove as of block timestamp {}",
                current_block_timestamp
            );
            return Ok(Vec::new());
        }

        tracing::info!(
            "After filtering invalid orders, found total of {} valid orders to proceed to locking and/or proving", 
            candidate_orders.len()
        );
        tracing::debug!(
            "Final orders ready for locking and/or proving after filtering: {}",
            candidate_orders.iter().map(|order| order.id()).collect::<Vec<_>>().join(", ")
        );

        Ok(candidate_orders)
    }

    fn prioritize_orders(&self, orders: &mut [Arc<OrderRequest>]) {
        // Sort orders by priority - for lock and fulfill orders, use lock expiration, for fulfill after lock expire, use request expiration
        orders.sort_by_key(|order| {
            if order.fulfillment_type == FulfillmentType::LockAndFulfill {
                order.request.lock_expires_at()
            } else {
                order.request.expires_at()
            }
        });

        tracing::debug!(
            "Orders ready for proving, prioritized. Before applying capacity limits: {:?}",
            orders
                .iter()
                .map(|order| format!(
                    "{} [Lock expires at: {}, Expires at: {}]",
                    order.id(),
                    order.request.lock_expires_at(),
                    order.request.expires_at()
                ))
                .collect::<Vec<_>>()
        );
    }

    async fn lock_and_prove_orders(&self, orders: &[Arc<OrderRequest>]) -> Result<()> {
        let mut processed_any = false;

        // Process new orders
        for order in orders {
            // Process the order
            let order_id = order.id();
            if order.fulfillment_type == FulfillmentType::LockAndFulfill {
                let request_id = order.request.id;
                match self.lock_order(order).await {
                    Ok(lock_price) => {
                        tracing::info!("Locked request: {request_id}");
                        if let Err(err) = self.db.insert_accepted_request(order, lock_price).await {
                            tracing::error!(
                                "FATAL STAKE AT RISK: {} failed to move from locking -> proving status {}",
                                order_id,
                                err
                            );
                        } else {
                            processed_any = true;
                        }
                    }
                    Err(ref err) => {
                        match err {
                            OrderMonitorErr::UnexpectedError(inner) => {
                                tracing::error!(
                                    "Failed to lock order: {order_id} - {} - {inner:?}",
                                    err.code()
                                );
                            }
                            _ => {
                                tracing::warn!(
                                    "Soft failed to lock request: {request_id} - {} - {err:?}",
                                    err.code()
                                );
                            }
                        }
                        if let Err(err) = self.db.insert_skipped_request(order).await {
                            tracing::error!(
                                "Failed to set DB failure state for order: {order_id} - {err:?}"
                            );
                        }
                    }
                }
                self.lock_and_prove_cache.invalidate(&order_id).await;
            } else {
                if let Err(err) = self.db.insert_accepted_request(order, U256::ZERO).await {
                    tracing::error!(
                        "Failed to set order status to pending proving: {} - {err:?}",
                        order_id
                    );
                } else {
                    processed_any = true;
                }
                self.prove_cache.invalidate(&order_id).await;
            }
        }

        // Log results
        if !processed_any && !orders.is_empty() {
            tracing::warn!("No new orders processed.");
        }

        Ok(())
    }

    /// Calculate the gas units needed for an order and the corresponding cost in wei
    async fn calculate_order_gas_cost_wei(
        &self,
        order: &OrderRequest,
        gas_price: u128,
    ) -> Result<U256, OrderMonitorErr> {
        // Calculate gas units needed for this order (lock + fulfill)
        let order_gas_units = if order.fulfillment_type == FulfillmentType::LockAndFulfill {
            U256::from(utils::estimate_gas_to_lock(&self.config, order).await?).saturating_add(
                U256::from(
                    utils::estimate_gas_to_fulfill(
                        &self.config,
                        &self.supported_selectors,
                        &order.request,
                    )
                    .await?,
                ),
            )
        } else {
            U256::from(
                utils::estimate_gas_to_fulfill(
                    &self.config,
                    &self.supported_selectors,
                    &order.request,
                )
                .await?,
            )
        };

        let order_cost_wei = U256::from(gas_price) * order_gas_units;

        Ok(order_cost_wei)
    }

    async fn apply_capacity_limits(
        &self,
        orders: Vec<Arc<OrderRequest>>,
        max_concurrent_proofs: Option<u32>,
        peak_prove_khz: Option<u64>,
    ) -> Result<Vec<Arc<OrderRequest>>> {
        let num_orders = orders.len();
        // Get our current capacity for proving orders given our config and the number of orders that are currently committed to be proven + fulfilled.
        let capacity = self.get_proving_order_capacity(max_concurrent_proofs).await?;
        let capacity_granted = capacity
            .request_capacity(num_orders.try_into().expect("Failed to convert order count to u32"));

        tracing::info!(
            "Current number of orders ready for locking and/or proving: {}. Total capacity available based on max_concurrent_proofs: {capacity:?}, Capacity granted this iteration: {capacity_granted:?}",
            num_orders
        );

        // Given our capacity computed from max_concurrent_proofs, truncate the order list.
        let mut orders_truncated = orders;
        if orders_truncated.len() > capacity_granted as usize {
            orders_truncated.truncate(capacity_granted as usize);
        }

        let mut final_orders: Vec<Arc<OrderRequest>> = Vec::with_capacity(orders_truncated.len());

        // Get current gas price and available balance
        let gas_price =
            self.chain_monitor.current_gas_price().await.context("Failed to get gas price")?;
        let available_balance_wei = self
            .provider
            .get_balance(self.provider.default_signer_address())
            .await
            .context("Failed to get current wallet balance")?;

        // Calculate gas units required for committed orders
        let committed_orders = self.db.get_committed_orders().await?;
        let committed_gas_units =
            futures::future::try_join_all(committed_orders.iter().map(|order| {
                utils::estimate_gas_to_fulfill(
                    &self.config,
                    &self.supported_selectors,
                    &order.request,
                )
            }))
            .await?
            .iter()
            .sum::<u64>();

        // Calculate cost in wei
        let committed_cost_wei = U256::from(gas_price) * U256::from(committed_gas_units);

        // Log committed order gas requirements
        if !committed_orders.is_empty() {
            tracing::debug!(
                "Cost for {} committed orders: {} ether",
                committed_orders.len(),
                format_ether(committed_cost_wei),
            );
        }

        // Ensure we have enough for committed orders
        if committed_cost_wei > available_balance_wei {
            tracing::error!(
                "Insufficient balance for committed orders. Required: {} ether, Available: {} ether",
                format_ether(committed_cost_wei),
                format_ether(available_balance_wei)
            );
            return Ok(Vec::new());
        }

        // Calculate remaining balance after accounting for committed orders
        let mut remaining_balance_wei = available_balance_wei - committed_cost_wei;

        // Apply peak khz limit if specified
        if peak_prove_khz.is_some() && !orders_truncated.is_empty() {
            let peak_prove_khz = peak_prove_khz.unwrap();
            let num_commited_orders = committed_orders.len();
            let total_commited_cycles =
                committed_orders.iter().map(|order| order.total_cycles.unwrap()).sum::<u64>();

            let now = now_timestamp();
            // Estimate the time the prover will be available given our current committed orders.
            let started_proving_at = committed_orders
                .iter()
                .map(|order| order.proving_started_at.unwrap_or(now))
                .min()
                .unwrap_or(now);

            let proof_time_seconds = total_commited_cycles.div_ceil(1_000).div_ceil(peak_prove_khz);
            let mut prover_available_at = started_proving_at + proof_time_seconds;
            if prover_available_at < now {
                tracing::warn!("Proofs are behind what is estimated from peak_prove_khz config. Consider lowering this value to avoid overlocking orders.");
                prover_available_at = now;
            }
            tracing::debug!("Already committed to {} orders, with a total cycle count of {}, a peak khz limit of {}, started working on them at {}, we estimate the prover will be available in {} seconds", 
                num_commited_orders,
                total_commited_cycles,
                peak_prove_khz,
                started_proving_at,
                prover_available_at.saturating_sub(now),
            );

            // For each order in consideration, check if it can be completed before its expiration.
            for order in orders_truncated {
                // Calculate gas and cost for this order using our helper method
                let order_cost_wei = self.calculate_order_gas_cost_wei(&order, gas_price).await?;

                // Skip if not enough balance
                if order_cost_wei > remaining_balance_wei {
                    tracing::warn!(
                        "Insufficient balance for order {}. Required: {} ether, Remaining: {} ether",
                        order.id(),
                        format_ether(order_cost_wei),
                        format_ether(remaining_balance_wei)
                    );
                    self.skip_order(&order, "insufficient balance").await;
                    continue;
                }

                if order.total_cycles.is_none() {
                    tracing::warn!("Order 0x{:x} has no total cycles, preflight was skipped? Not considering for peak khz limit", order.request.id);
                    final_orders.push(order);
                    remaining_balance_wei -= order_cost_wei;
                    continue;
                }

                let proof_time_seconds =
                    order.total_cycles.unwrap().div_ceil(1_000).div_ceil(peak_prove_khz);
                let completion_time = prover_available_at + proof_time_seconds;
                let expiration = match order.fulfillment_type {
                    FulfillmentType::LockAndFulfill => order.request.lock_expires_at(),
                    FulfillmentType::FulfillAfterLockExpire => order.request.expires_at(),
                    _ => panic!("Unsupported fulfillment type: {:?}", order.fulfillment_type),
                };

                tracing::debug!("Order {} estimated to take {} seconds, and would be completed at {} ({} seconds from now). It expires at {} ({} seconds from now)", order.id(), proof_time_seconds, completion_time, completion_time.saturating_sub(now_timestamp()), expiration, expiration.saturating_sub(now_timestamp()));

                if completion_time > expiration {
                    tracing::info!("Order 0x{:x} cannot be completed before its expiration at {}, proof estimated to take {} seconds and complete at {}. Skipping", 
                        order.request.id,
                        expiration,
                        proof_time_seconds,
                        completion_time
                    );
                    // Set order to skipped in DB and continue.
                    self.skip_order(&order, "peak KHz limit: completion past expiration").await;
                    continue;
                }

                final_orders.push(order);
                prover_available_at = completion_time;
                remaining_balance_wei -= order_cost_wei;
            }
        } else {
            // If no peak khz limit, just check gas for each order
            for order in orders_truncated {
                let order_cost_wei = self.calculate_order_gas_cost_wei(&order, gas_price).await?;

                // Skip if not enough balance
                if order_cost_wei > remaining_balance_wei {
                    tracing::warn!(
                        "Insufficient balance for order {}. Required: {} ether, Remaining: {} ether",
                        order.id(),
                        format_ether(order_cost_wei),
                        format_ether(remaining_balance_wei)
                    );
                    self.skip_order(&order, "insufficient balance").await;
                    continue;
                }

                final_orders.push(order);
                remaining_balance_wei -= order_cost_wei;
            }
        }

        tracing::info!(
            "Started with {} orders ready to be locked and/or proven. After applying capacity limits of {} max concurrent proofs and {} peak khz, filtered to {} orders: {:?}",
            num_orders,
            if let Some(max_concurrent_proofs) = max_concurrent_proofs {
                max_concurrent_proofs.to_string()
            } else {
                "unlimited".to_string()
            },
            if let Some(peak_prove_khz) = peak_prove_khz {
                peak_prove_khz.to_string()
            } else {
                "unlimited".to_string()
            },
            final_orders.len(),
            final_orders.iter().map(|order| order.id()).collect::<Vec<_>>()
        );

        Ok(final_orders)
    }

    pub async fn start_monitor(self) -> Result<(), OrderMonitorErr> {
        let mut last_block = 0;
        let mut first_block = 0;
        let mut interval = tokio::time::interval_at(
            tokio::time::Instant::now(),
            tokio::time::Duration::from_secs(self.block_time),
        );
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        let mut new_orders = self.priced_order_rx.lock().await;

        loop {
            tokio::select! {
                // Process new orders from the channel as soon as they arrive
                biased;

                Some(result) = new_orders.recv() => {
                    self.handle_new_order_result(result).await?;
                }

                // On each interval, process all pending orders and do the block-based logic
                _ = interval.tick() => {
                    let ChainHead { block_number, block_timestamp } =
                        self.chain_monitor.current_chain_head().await?;
                    if block_number != last_block {
                        last_block = block_number;
                        if first_block == 0 {
                            first_block = block_number;
                        }
                        tracing::trace!(
                            "Order monitor processing block {block_number} at timestamp {block_timestamp}"
                        );

                        let (min_deadline, peak_prove_khz, max_concurrent_proofs) = {
                            let config = self.config.lock_all().context("Failed to read config")?;
                            (
                                config.market.min_deadline,
                                config.market.peak_prove_khz,
                                config.market.max_concurrent_proofs,
                            )
                        };

                        // Get orders that are valid and ready for locking/proving, skipping orders that are now invalid for proving, due to expiring, being locked by another prover, etc.
                        let mut valid_orders = self.get_valid_orders(block_timestamp, min_deadline).await?;

                        if valid_orders.is_empty() {
                            tracing::trace!(
                                "No orders to lock and/or prove as of block timestamp {}",
                                block_timestamp
                            );
                            continue;
                        }

                        // Prioritize the orders that intend to fulfill based on when they need to locked and/or proven.
                        self.prioritize_orders(&mut valid_orders);

                        // Filter down the orders given our max concurrent proofs, peak khz limits, and gas limitations.
                        let final_orders = self
                            .apply_capacity_limits(
                                valid_orders,
                                max_concurrent_proofs,
                                peak_prove_khz,
                            )
                            .await?;

                        tracing::debug!("After processing block {}[timestamp {}], we will now start locking and/or proving {} orders.",
                            block_number,
                            block_timestamp,
                            final_orders.len(),
                        );

                        if !final_orders.is_empty() {
                            // Lock and prove filtered orders.
                            self.lock_and_prove_orders(&final_orders).await?;
                        }
                    }
                }
            }
        }
    }

    // Called when a new order result is received from the channel
    async fn handle_new_order_result(
        &self,
        order: Box<OrderRequest>,
    ) -> Result<(), OrderMonitorErr> {
        match order.fulfillment_type {
            FulfillmentType::LockAndFulfill => {
                // Note: this could be done without waiting for the batch to minimize latency, but
                //       avoiding more complicated logic for checking capacity for each order.

                // If not, add it to the cache to be locked after target time
                self.lock_and_prove_cache.insert(order.id(), Arc::from(order)).await;
            }
            FulfillmentType::FulfillAfterLockExpire | FulfillmentType::FulfillWithoutLocking => {
                self.prove_cache.insert(order.id(), Arc::from(order)).await;
            }
        }
        Ok(())
    }
}

impl<P> RetryTask for OrderMonitor<P>
where
    P: Provider<Ethereum> + WalletProvider + 'static + Clone,
{
    type Error = OrderMonitorErr;
    fn spawn(&self) -> RetryRes<Self::Error> {
        let monitor_clone = self.clone();
        Box::pin(async move {
            tracing::info!("Starting order monitor");
            monitor_clone.start_monitor().await.map_err(SupervisorErr::Recover)?;
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::OrderStatus;
    use crate::{db::SqliteDb, now_timestamp, FulfillmentType};
    use alloy::node_bindings::AnvilInstance;
    use alloy::{
        network::EthereumWallet,
        node_bindings::Anvil,
        primitives::{Address, U256},
        providers::ProviderBuilder,
        signers::local::PrivateKeySigner,
    };
    use boundless_market::contracts::{
        Input, InputType, Offer, Predicate, PredicateType, ProofRequest, RequestId, Requirements,
    };
    use boundless_market_test_utils::{
        deploy_boundless_market, deploy_hit_points, ASSESSOR_GUEST_ID, ASSESSOR_GUEST_PATH,
    };

    use risc0_zkvm::Digest;
    use std::{future::Future, sync::Arc};
    use tokio::task::JoinSet;
    use tracing_test::traced_test;

    type TestProvider = alloy::providers::fillers::FillProvider<
        alloy::providers::fillers::JoinFill<
            alloy::providers::fillers::JoinFill<
                alloy::providers::Identity,
                alloy::providers::fillers::JoinFill<
                    alloy::providers::fillers::GasFiller,
                    alloy::providers::fillers::JoinFill<
                        alloy::providers::fillers::BlobGasFiller,
                        alloy::providers::fillers::JoinFill<
                            alloy::providers::fillers::NonceFiller,
                            alloy::providers::fillers::ChainIdFiller,
                        >,
                    >,
                >,
            >,
            alloy::providers::fillers::WalletFiller<EthereumWallet>,
        >,
        alloy::providers::RootProvider,
    >;

    struct TestCtx {
        monitor: OrderMonitor<TestProvider>,
        anvil: AnvilInstance,
        db: DbObj,
        market_address: Address,
        #[allow(dead_code)]
        config: ConfigLock,
        priced_order_tx: mpsc::Sender<Box<OrderRequest>>,
        signer: PrivateKeySigner,
        market_service: BoundlessMarketService<Arc<TestProvider>>,
        next_order_id: u32, // Counter to assign unique order IDs
    }

    impl TestCtx {
        // Convert the standalone function to a method on TestCtx
        async fn create_test_order(
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
                Requirements::new(
                    Digest::ZERO,
                    Predicate {
                        predicateType: PredicateType::PrefixMatch,
                        data: Default::default(),
                    },
                ),
                "http://risczero.com/image",
                Input { inputType: InputType::Inline, data: Default::default() },
                Offer {
                    minPrice: U256::from(1),
                    maxPrice: U256::from(2),
                    biddingStart: bidding_start,
                    rampUpPeriod: 1,
                    timeout: timeout as u32,
                    lockTimeout: lock_timeout as u32,
                    lockStake: U256::from(0),
                },
            );

            let client_sig = request
                .sign_request(&self.signer, self.market_address, self.anvil.chain_id())
                .await
                .unwrap()
                .as_bytes()
                .into();

            Box::new(OrderRequest {
                target_timestamp: Some(0),
                request,
                image_id: None,
                input_id: None,
                expire_timestamp: None,
                client_sig,
                fulfillment_type,
                boundless_market_address: self.market_address,
                chain_id: self.anvil.chain_id(),
                total_cycles: None,
            })
        }
    }

    async fn setup_test() -> TestCtx {
        let anvil = Anvil::new().spawn();
        let signer: PrivateKeySigner = anvil.keys()[0].clone().into();
        let provider = Arc::new(
            ProviderBuilder::new()
                .wallet(EthereumWallet::from(signer.clone()))
                .connect(&anvil.endpoint())
                .await
                .unwrap(),
        );

        // Deploy contracts
        let hit_points = deploy_hit_points(signer.address(), provider.clone()).await.unwrap();

        let market_address = deploy_boundless_market(
            signer.address(),
            provider.clone(),
            Address::ZERO,
            hit_points,
            Digest::from(ASSESSOR_GUEST_ID),
            format!("file://{ASSESSOR_GUEST_PATH}"),
            Some(signer.address()),
        )
        .await
        .unwrap();

        // Set up market service
        let market_service = BoundlessMarketService::new(
            market_address,
            provider.clone(),
            provider.default_signer_address(),
        );

        // Deposit ETH into the contract for the prover to use when locking orders
        // Using 10 ETH to ensure plenty of funds for tests
        market_service.deposit(parse_ether("10.0").unwrap()).await.unwrap();

        let db: DbObj = Arc::new(SqliteDb::new("sqlite::memory:").await.unwrap());
        let config = ConfigLock::default();

        config.load_write().unwrap().market.min_deadline = 50;
        config.load_write().unwrap().market.lockin_gas_estimate = 200_000;
        config.load_write().unwrap().market.fulfill_gas_estimate = 300_000;
        config.load_write().unwrap().market.groth16_verify_gas_estimate = 50_000;

        let block_time = 2;

        let chain_monitor = Arc::new(ChainMonitorService::new(provider.clone()).await.unwrap());
        tokio::spawn(chain_monitor.spawn());

        // Create required channels for tests
        let (priced_order_tx, priced_order_rx) = mpsc::channel(16);

        let monitor = OrderMonitor::new(
            db.clone(),
            provider.clone(),
            chain_monitor.clone(),
            config.clone(),
            block_time,
            signer.address(),
            market_address,
            priced_order_rx,
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
            next_order_id: 1, // Initialize with 1 instead of 0
        }
    }

    async fn run_with_monitor<P, F, T>(monitor: OrderMonitor<P>, f: F) -> T
    where
        P: Provider + WalletProvider + Clone + 'static,
        F: Future<Output = T>,
    {
        // A JoinSet automatically aborts all its tasks when dropped
        let mut tasks = JoinSet::new();
        // Spawn the monitor
        tasks.spawn(async move { monitor.start_monitor().await });

        tokio::select! {
            result = f => result,
            monitor_task_result = tasks.join_next() => {
                panic!("Monitor exited unexpectedly: {:?}", monitor_task_result.unwrap());
            },
        }
    }

    #[tokio::test]
    #[traced_test]
    async fn monitor_block() {
        let mut ctx = setup_test().await;

        // Create a test order using the TestCtx helper method
        let order =
            ctx.create_test_order(FulfillmentType::LockAndFulfill, now_timestamp(), 100, 200).await;

        let order_id = order.id();

        let _request_id =
            ctx.market_service.submit_request(&order.request, &ctx.signer).await.unwrap();

        // Send the order to the monitor
        ctx.priced_order_tx.send(order).await.unwrap();

        run_with_monitor(ctx.monitor, async move {
            // loop for 20 seconds
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

    // Capacity tests
    #[test]
    fn test_capacity_unlimited() {
        let capacity = Capacity::Unlimited;
        assert_eq!(capacity.request_capacity(0), 0);
        assert_eq!(capacity.request_capacity(15), MAX_PROVING_BATCH_SIZE);
        assert_eq!(capacity.request_capacity(MAX_PROVING_BATCH_SIZE), MAX_PROVING_BATCH_SIZE);
    }

    #[test]
    fn test_capacity_proving() {
        let capacity = Capacity::Proving(50);
        assert_eq!(capacity.request_capacity(0), 0);
        assert_eq!(capacity.request_capacity(4), 4);
        assert_eq!(capacity.request_capacity(10), MAX_PROVING_BATCH_SIZE);
    }

    // Filtering tests
    #[tokio::test]
    #[traced_test]
    async fn test_filter_expired_orders() {
        let mut ctx = setup_test().await;
        let current_timestamp = now_timestamp();

        // Create an expired order
        let expired_order = ctx
            .create_test_order(FulfillmentType::LockAndFulfill, current_timestamp - 100, 50, 50)
            .await;
        let expired_order_id = expired_order.id();
        ctx.monitor
            .lock_and_prove_cache
            .insert(expired_order_id.clone(), Arc::from(expired_order))
            .await;

        let result = ctx.monitor.get_valid_orders(current_timestamp, 0).await.unwrap();

        assert!(result.is_empty());

        let order = ctx.db.get_order(&expired_order_id).await.unwrap().unwrap();
        assert_eq!(order.status, OrderStatus::Skipped);
    }

    #[tokio::test]
    #[traced_test]
    async fn test_filter_insufficient_deadline() {
        let mut ctx = setup_test().await;
        let current_timestamp = now_timestamp();

        // Create an order with insufficient deadline
        let order =
            ctx.create_test_order(FulfillmentType::LockAndFulfill, current_timestamp, 45, 45).await;
        let order_1_id = order.id();
        ctx.monitor.lock_and_prove_cache.insert(order_1_id.clone(), Arc::from(order)).await;

        // Create an order with insufficient deadline
        let order = ctx
            .create_test_order(FulfillmentType::FulfillAfterLockExpire, current_timestamp, 1, 45)
            .await;
        let order_2_id = order.id();
        ctx.monitor.prove_cache.insert(order_2_id.clone(), Arc::from(order)).await;

        let result = ctx.monitor.get_valid_orders(current_timestamp, 100).await.unwrap();

        assert!(result.is_empty());

        let order = ctx.db.get_order(&order_1_id).await.unwrap().unwrap();
        assert_eq!(order.status, OrderStatus::Skipped);

        let order = ctx.db.get_order(&order_2_id).await.unwrap().unwrap();
        assert_eq!(order.status, OrderStatus::Skipped);
    }

    #[tokio::test]
    async fn test_filter_locked_by_others() {
        let mut ctx = setup_test().await;
        let current_timestamp = now_timestamp();

        // Create an order that's locked by another prover
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
        ctx.monitor.lock_and_prove_cache.insert(order.id(), Arc::from(order)).await;

        let result =
            ctx.monitor.get_valid_orders(current_timestamp, current_timestamp + 100).await.unwrap();

        assert!(result.is_empty());

        let order = ctx.db.get_order(&order_id).await.unwrap().unwrap();
        assert_eq!(order.status, OrderStatus::Skipped);
    }

    // Sorting tests
    #[tokio::test]
    async fn test_prioritize_orders() {
        let mut ctx = setup_test().await;
        let current_timestamp = now_timestamp();

        // Create orders with different expiration times
        // Must lock and fulfill within 50 seconds
        let order1 = ctx
            .create_test_order(FulfillmentType::LockAndFulfill, current_timestamp, 50, 200)
            .await;
        let order_1_id = order1.id();

        // Must lock and fulfill within 100 seconds.
        let order2 = ctx
            .create_test_order(FulfillmentType::LockAndFulfill, current_timestamp, 100, 200)
            .await;
        let order_2_id = order2.id();

        // Must fulfill after lock expires within 51 seconds.
        let order3 = ctx
            .create_test_order(FulfillmentType::FulfillAfterLockExpire, current_timestamp, 1, 51)
            .await;
        let order_3_id = order3.id();

        // Must fulfill after lock expires within 53 seconds.
        let order4 = ctx
            .create_test_order(FulfillmentType::FulfillAfterLockExpire, current_timestamp, 1, 53)
            .await;
        let order_4_id = order4.id();

        let mut orders =
            vec![Arc::from(order1), Arc::from(order2), Arc::from(order3), Arc::from(order4)];
        ctx.monitor.prioritize_orders(&mut orders);

        assert!(orders[0].id() == order_1_id);
        assert!(orders[1].id() == order_3_id);
        assert!(orders[2].id() == order_4_id);
        assert!(orders[3].id() == order_2_id);
    }

    // Processing tests
    #[tokio::test]
    #[traced_test]
    async fn test_process_fulfill_after_lock_expire_orders() {
        let mut ctx = setup_test().await;
        let current_timestamp = now_timestamp();

        let order = ctx
            .create_test_order(FulfillmentType::FulfillAfterLockExpire, current_timestamp, 100, 200)
            .await;
        let order_id = order.id();

        ctx.monitor.lock_and_prove_orders(&[Arc::from(order)]).await.unwrap();

        let updated_order = ctx.db.get_order(&order_id).await.unwrap().unwrap();
        assert_eq!(updated_order.status, OrderStatus::PendingProving);
    }

    #[tokio::test]
    #[traced_test]
    async fn test_apply_capacity_limits_unlimited() {
        let mut ctx = setup_test().await;
        let current_timestamp = now_timestamp();

        // Create multiple orders
        let mut orders = Vec::new();
        for _ in 1..=5 {
            let order = ctx
                .create_test_order(FulfillmentType::LockAndFulfill, current_timestamp, 100, 200)
                .await;

            // Submit requests to blockchain
            let _request_id =
                ctx.market_service.submit_request(&order.request, &ctx.signer).await.unwrap();

            orders.push(Arc::from(order));
        }

        // Set unlimited capacity in config
        ctx.config.load_write().unwrap().market.max_concurrent_proofs = None;

        // Process all orders with unlimited capacity
        let filtered_orders =
            ctx.monitor.apply_capacity_limits(orders.clone(), None, None).await.unwrap();
        let result = ctx.monitor.lock_and_prove_orders(&filtered_orders).await;
        assert!(result.is_ok(), "lock_and_prove_orders should succeed");

        // All orders should be processed since capacity is unlimited
        let mut processed_count = 0;
        for order in orders {
            if let Some(order) = ctx.db.get_order(&order.id()).await.unwrap() {
                processed_count += 1;
                assert_eq!(order.status, OrderStatus::PendingProving);
            }
        }

        // Should process all 5 orders
        assert_eq!(
            processed_count, 5,
            "Should have processed all 5 orders with unlimited capacity"
        );
    }

    #[tokio::test]
    #[traced_test]
    async fn test_apply_capacity_limits_proving() {
        let mut ctx = setup_test().await;
        let current_timestamp = now_timestamp();

        // Add a committed order to simulate existing workload
        let committed_order = ctx
            .create_test_order(FulfillmentType::LockAndFulfill, current_timestamp, 100, 200)
            .await;
        let mut committed_order = committed_order.to_proving_order(Default::default());
        committed_order.status = OrderStatus::Proving;
        committed_order.proving_started_at = Some(current_timestamp);
        ctx.db.add_order(&committed_order).await.unwrap();

        // Create multiple new orders
        let mut orders = Vec::new();
        for _ in 1..=5 {
            let order = ctx
                .create_test_order(FulfillmentType::LockAndFulfill, current_timestamp, 100, 200)
                .await;

            // Submit requests to blockchain
            let _request_id =
                ctx.market_service.submit_request(&order.request, &ctx.signer).await.unwrap();

            orders.push(Arc::from(order));
        }

        // Process orders with limited capacity
        let orders = ctx.monitor.apply_capacity_limits(orders, Some(3), None).await.unwrap();
        ctx.monitor.lock_and_prove_orders(&orders).await.unwrap();

        // Count processed orders
        let mut processed_count = 0;
        for order in orders {
            if let Some(order) = ctx.db.get_order(&order.id()).await.unwrap() {
                processed_count += 1;
                assert_eq!(order.status, OrderStatus::PendingProving);
            }
        }

        // Should only process 2 more orders (3 total with 1 already committed)
        assert_eq!(
            processed_count, 2,
            "Should have processed only 2 more orders due to concurrent proving capacity limit"
        );
    }

    #[tokio::test]
    #[traced_test]
    async fn test_apply_capacity_limits_committed_work_too_large() {
        let mut ctx = setup_test().await;
        let current_timestamp = now_timestamp();

        // Add a large committed order to simulate existing workload
        let committed_order = ctx
            .create_test_order(FulfillmentType::LockAndFulfill, current_timestamp, 100, 200)
            .await;
        let mut committed_order = committed_order.to_proving_order(Default::default());
        committed_order.status = OrderStatus::Proving;
        committed_order.total_cycles = Some(10_000_000_000_000_000);
        committed_order.proving_started_at = Some(current_timestamp);
        ctx.db.add_order(&committed_order).await.unwrap();

        let mut orders = Vec::new();

        let mut order1 = ctx
            .create_test_order(FulfillmentType::LockAndFulfill, current_timestamp, 100, 200)
            .await;
        order1.total_cycles = Some(1000);

        orders.push(Arc::from(order1));

        let mut order2 = ctx
            .create_test_order(FulfillmentType::LockAndFulfill, current_timestamp, 100, 200)
            .await;
        order2.total_cycles = Some(100);
        orders.push(Arc::from(order2));

        let orders = ctx.monitor.apply_capacity_limits(orders, None, Some(100)).await.unwrap();

        assert_eq!(orders.len(), 0);
        assert!(logs_contain("cannot be completed before its expiration"));
        assert!(logs_contain("Started with 2 orders"));
        assert!(logs_contain("filtered to 0 orders: []"));
    }

    #[tokio::test]
    async fn test_apply_capacity_limits_skip_proof_time_past_expiration() {
        let mut ctx = setup_test().await;
        let current_timestamp = now_timestamp();

        // Create orders with different expiration times
        let mut candidate_orders = Vec::new();

        // Order 1: Will expire soon (not enough time to prove)
        let mut order1 =
            ctx.create_test_order(FulfillmentType::LockAndFulfill, current_timestamp, 5, 5).await;
        order1.total_cycles = Some(1000000000000);
        let order1_id = order1.id();

        let _request_id =
            ctx.market_service.submit_request(&order1.request, &ctx.signer).await.unwrap();
        candidate_orders.push(Arc::from(order1));

        // Order 2: Longer expiration (enough time to prove)
        let mut order2 = ctx
            .create_test_order(FulfillmentType::LockAndFulfill, current_timestamp, 100, 200)
            .await;
        order2.total_cycles = Some(2000);
        let order2_id = order2.id();
        let _request_id =
            ctx.market_service.submit_request(&order2.request, &ctx.signer).await.unwrap();
        candidate_orders.push(Arc::from(order2));

        let filtered_orders =
            ctx.monitor.apply_capacity_limits(candidate_orders, None, Some(1)).await.unwrap();

        assert_eq!(filtered_orders[0].total_cycles, Some(2000));
        assert_eq!(filtered_orders[0].id(), order2_id);

        // The first order should be skipped due to insufficient proof time before expiration
        let order1_db = ctx.db.get_order(&order1_id).await.unwrap();
        assert_eq!(
            order1_db.unwrap().status,
            OrderStatus::Skipped,
            "Order 1 should be skipped due to insufficient time to complete proof"
        );
    }

    #[tokio::test]
    async fn test_gas_estimation_functions() {
        let mut ctx = setup_test().await;

        // Create orders with different fulfillment types to test gas estimation for each type
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
        let filtered_orders = ctx.monitor.apply_capacity_limits(orders, None, None).await.unwrap();
        let result = ctx.monitor.lock_and_prove_orders(&filtered_orders).await;
        assert!(result.is_ok(), "lock_and_prove_orders should succeed");

        // Verify both orders were processed correctly
        let lock_order_result = ctx.db.get_order(&lock_and_fulfill_id).await.unwrap();
        let fulfill_order_result = ctx.db.get_order(&fulfill_only_id).await.unwrap();

        assert!(lock_order_result.is_some(), "Lock and fulfill order should be processed");
        assert!(fulfill_order_result.is_some(), "Fulfill only order should be processed");

        assert_eq!(lock_order_result.unwrap().status, OrderStatus::PendingProving);
        assert_eq!(fulfill_order_result.unwrap().status, OrderStatus::PendingProving);
    }

    #[tokio::test]
    #[traced_test]
    async fn test_multiple_orders_khz_capacity() {
        let mut ctx = setup_test().await;
        ctx.config.load_write().unwrap().market.max_concurrent_proofs = None;

        // Create multiple orders with increasing cycle counts to test gas allocation
        let mut orders = Vec::new();
        for i in 1..6 {
            let mut order = ctx
                .create_test_order(FulfillmentType::LockAndFulfill, now_timestamp(), 120, 120)
                .await;

            // Set increasing cycle counts to test different gas requirements
            order.total_cycles = Some(i as u64 * 1_000_000);

            let _request_id =
                ctx.market_service.submit_request(&order.request, &ctx.signer).await.unwrap();

            orders.push(Arc::from(order));
        }

        let filtered_orders =
            ctx.monitor.apply_capacity_limits(orders, None, Some(100)).await.unwrap();

        println!("filtered_orders: {:?}", filtered_orders);
        // 100khz can prove 1m+2m+3m+4m (10m) cycles in 100 seconds
        assert_eq!(filtered_orders.len(), 4);

        assert_eq!(filtered_orders[0].total_cycles, Some(1_000_000));
        assert_eq!(filtered_orders[3].total_cycles, Some(4_000_000));
    }

    #[tokio::test]
    #[traced_test]
    async fn test_insufficient_balance_committed_orders() {
        let mut ctx = setup_test().await;

        let balance = ctx.monitor.provider.get_balance(ctx.signer.address()).await.unwrap();
        let gas_price = ctx.monitor.provider.get_gas_price().await.unwrap();
        let gas_remaining: u64 = (balance / U256::from(gas_price)).try_into().unwrap();
        ctx.config.load_write().unwrap().market.fulfill_gas_estimate = gas_remaining / 2;
        ctx.config.load_write().unwrap().market.lockin_gas_estimate = gas_remaining / 3;

        let incoming_order =
            ctx.create_test_order(FulfillmentType::LockAndFulfill, now_timestamp(), 100, 200).await;

        let mut orders = vec![Arc::from(incoming_order)];

        // Should be able to have enough gas for 1 lock and fulfill
        let filtered_orders =
            ctx.monitor.apply_capacity_limits(orders.clone(), None, None).await.unwrap();
        assert_eq!(filtered_orders.len(), 1);

        orders.push(Arc::from(
            ctx.create_test_order(FulfillmentType::LockAndFulfill, now_timestamp(), 100, 200).await,
        ));

        // Should still only be able to have enough gas for 1 lock and fulfill
        let filtered_orders =
            ctx.monitor.apply_capacity_limits(orders.clone(), None, None).await.unwrap();
        assert_eq!(filtered_orders.len(), 1);

        for _ in 0..3 {
            let committed_order = ctx
                .create_test_order(FulfillmentType::LockAndFulfill, now_timestamp(), 100, 200)
                .await;

            let mut committed_order_obj = committed_order.to_proving_order(Default::default());
            committed_order_obj.status = OrderStatus::Proving;
            committed_order_obj.proving_started_at = Some(now_timestamp());
            ctx.db.add_order(&committed_order_obj).await.unwrap();
        }

        // Process the order - with insufficient balance for committed orders
        let filtered_orders = ctx.monitor.apply_capacity_limits(orders, None, None).await.unwrap();

        assert!(filtered_orders.is_empty());
    }
}
