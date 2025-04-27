// Copyright (c) 2025 RISC Zero, Inc.
//
// All rights reserved.

use crate::{
    chain_monitor::ChainMonitorService,
    config::ConfigLock,
    db::DbObj,
    now_timestamp,
    task::{RetryRes, RetryTask, SupervisorErr},
    FulfillmentType, Order, OrderStatus,
};
use alloy::{
    network::Ethereum,
    primitives::{utils::parse_ether, Address, U256},
    providers::{Provider, WalletProvider},
};
use anyhow::{Context, Result};
use boundless_market::contracts::{
    boundless_market::{BoundlessMarketService, MarketError},
    RequestStatus,
};
use std::{sync::Arc, time::Duration};
use thiserror::Error;

/// Maximum number of orders to concurrently work on proving.
const MAX_PROVING_BATCH_SIZE: u32 = 10;

#[derive(Error, Debug)]
pub enum LockOrderErr {
    #[error("Failed to fetch / push image: {0}")]
    OrderLockedInBlock(MarketError),

    #[error("Invalid order status for locking: {0:?}")]
    InvalidStatus(OrderStatus),

    #[error("Order already locked")]
    AlreadyLocked,

    #[error("Other: {0}")]
    OtherErr(#[from] anyhow::Error),
}

/// The capacity of the order monitor, if there is a limit on the number of concurrent proving tasks.
#[derive(Debug, PartialEq)]
enum Capacity {
    /// There are orders that have been picked for proving but not fulfilled yet.
    /// Number indicates available slots.
    Proving(u32),
    /// There is no concurrent lock limit.
    Unlimited,
}

impl Capacity {
    /// Returns the number of orders to request from the DB to price. Capped at
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

struct OrdersByFulfillmentType {
    lock_and_prove_orders: Vec<Order>,
    prove_orders: Vec<Order>,
}

impl OrdersByFulfillmentType {
    fn len(&self) -> usize {
        self.lock_and_prove_orders.len() + self.prove_orders.len()
    }
}

impl std::fmt::Debug for OrdersByFulfillmentType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut all_orders: Vec<_> = self.lock_and_prove_orders.iter().map(|o| o.id()).collect();
        all_orders.extend(self.prove_orders.iter().map(|o| o.id()));
        f.debug_struct("OrdersByFulfillmentType")
            .field("total_orders", &(self.lock_and_prove_orders.len() + self.prove_orders.len()))
            .field("lock_and_prove_orders", &self.lock_and_prove_orders.len())
            .field("prove_orders", &self.prove_orders.len())
            .field("order_ids", &all_orders)
            .finish()
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
    capacity_debug_log: String,
}

impl<P> OrderMonitor<P>
where
    P: Provider + WalletProvider,
{
    pub fn new(
        db: DbObj,
        provider: Arc<P>,
        chain_monitor: Arc<ChainMonitorService<P>>,
        config: ConfigLock,
        block_time: u64,
        market_addr: Address,
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
            let config = config.lock_all().context("Failed to lock config")?;
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

        Ok(Self {
            db,
            chain_monitor,
            block_time,
            config,
            market,
            provider,
            capacity_debug_log: "".to_string(),
        })
    }

    async fn lock_order(&self, order: &Order) -> Result<(), LockOrderErr> {
        if order.status != OrderStatus::WaitingToLock {
            return Err(LockOrderErr::InvalidStatus(order.status));
        }

        let request_id = order.request.id;

        let order_status = self
            .market
            .get_status(request_id, Some(order.request.expires_at()))
            .await
            .context("Failed to get request status")?;
        if order_status != RequestStatus::Unknown {
            tracing::warn!("Request {} not open: {order_status:?}, skipping", request_id);
            // TODO: fetch some chain data to find out who / and for how much the order
            // was locked in at
            return Err(LockOrderErr::AlreadyLocked);
        }

        let is_locked = self
            .db
            .is_request_locked(U256::from(order.request.id))
            .await
            .context("Failed to check if request is locked")?;
        if is_locked {
            tracing::warn!("Request {} already locked: {order_status:?}, skipping", request_id);
            return Err(LockOrderErr::AlreadyLocked);
        }

        let conf_priority_gas = {
            let conf = self.config.lock_all().context("Failed to lock config")?;
            conf.market.lockin_priority_gas
        };

        tracing::info!(
            "Locking request: {} for stake: {}",
            request_id,
            order.request.offer.lockStake
        );
        let lock_block = self
            .market
            .lock_request(&order.request, &order.client_sig, conf_priority_gas)
            .await
            .map_err(LockOrderErr::OrderLockedInBlock)?;

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

        self.db
            .set_proving_status_lock_and_fulfill_orders(&order.id(), lock_price)
            .await
            .with_context(|| {
                format!(
                    "FATAL STAKE AT RISK: {} failed to move from locking -> proving status",
                    order.id()
                )
            })?;

        Ok(())
    }

    async fn lock_orders(&self, current_block: u64, orders: Vec<Order>) -> Result<u64> {
        let mut order_count = 0;
        for order in orders.iter() {
            let order_id = order.id();
            let request_id = order.request.id;
            match self.lock_order(order).await {
                Ok(_) => tracing::info!("Locked request: {request_id}"),
                Err(ref err) => {
                    match err {
                        LockOrderErr::OtherErr(err) => {
                            tracing::error!("Failed to lock request: {request_id} {err:?}");
                        }
                        // Only warn on known / classified errors
                        _ => {
                            tracing::warn!("Soft failed to lock request: {request_id} {err:?}");
                        }
                    }
                    if let Err(err) = self.db.set_order_failure(&order_id, format!("{err:?}")).await
                    {
                        tracing::error!(
                            "Failed to set DB failure state for order: {order_id}, {err:?}"
                        );
                    }
                }
            }
            order_count += 1;
        }

        if !orders.is_empty() {
            self.db
                .set_last_block(current_block)
                .await
                .context("Failed to update db last block")?;
        }

        Ok(order_count)
    }

    async fn back_scan_locks(&self) -> Result<u64> {
        let opt_last_block =
            self.db.get_last_block().await.context("Failed to fetch last block from DB")?;

        // back scan if we have an existing block we last updated from
        // TODO: spawn a side thread to avoid missing new blocks while this is running:
        let order_count = if opt_last_block.is_some() {
            let current_block = self.chain_monitor.current_block_number().await?;
            let current_block_timestamp = self.chain_monitor.current_block_timestamp().await?;

            tracing::debug!(
                "Checking status of, and locking, orders marked as pending lock at block {current_block} @ {current_block_timestamp}"
            );

            // Get the orders that we wish to lock as early as the next block.
            let orders = self
                .db
                .get_pending_lock_orders(current_block_timestamp + self.block_time)
                .await
                .context("Failed to find pending lock orders")?;

            self.lock_orders(current_block, orders).await.context("Failed to lock orders")?
        } else {
            0
        };

        Ok(order_count)
    }

    async fn get_proving_order_capacity(
        &mut self,
        max_concurrent_proofs: Option<u32>,
    ) -> Result<Capacity> {
        if max_concurrent_proofs.is_none() {
            return Ok(Capacity::Unlimited);
        };

        let max = max_concurrent_proofs.unwrap();
        let committed_orders = self.db.get_committed_orders().await?;
        let committed_orders_count: u32 = committed_orders.len().try_into().unwrap();

        self.log_capacity(committed_orders, max).await;

        let available_slots = max.saturating_sub(committed_orders_count);
        Ok(Capacity::Proving(available_slots))
    }

    async fn log_capacity(&mut self, commited_orders: Vec<Order>, max: u32) {
        let committed_orders_count: u32 = commited_orders.len().try_into().unwrap();
        let request_id_and_status = commited_orders
            .iter()
            .map(|order| {
                (
                    format!("{:x}", order.request.id),
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
        if self.capacity_debug_log != capacity_debug_log {
            tracing::info!("{}", capacity_debug_log);
            self.capacity_debug_log = capacity_debug_log;
        }
    }

    async fn prove_orders(&self, orders: Vec<Order>) -> Result<()> {
        for order in orders {
            self.db
                .set_proving_status_fulfill_after_lock_expire_orders(&order.id())
                .await
                .context("Failed to set order status to pending proving")?;
        }
        Ok(())
    }

    async fn get_valid_orders(
        &self,
        current_block_timestamp: u64,
        min_deadline: u64,
    ) -> Result<Vec<Order>> {
        let mut candidate_orders: Vec<Order> = Vec::new();

        // Find the orders that we intended to prove after their lock expires
        let lock_expired_orders = self
            .db
            .get_fulfill_after_lock_expire_orders(current_block_timestamp)
            .await
            .context("Failed to find pending prove after lock expire orders")?;

        tracing::trace!(
            "Found orders that we intend to prove after their lock expires: {:?}",
            lock_expired_orders.iter().map(|order| order.id()).collect::<Vec<_>>()
        );

        for order in lock_expired_orders {
            let is_fulfilled = self
                .db
                .is_request_fulfilled(U256::from(order.request.id))
                .await
                .context("Failed to check if request is fulfilled")?;
            if is_fulfilled {
                tracing::info!(
                    "Request {:x} was locked by another prover and was fulfilled. Skipping.",
                    order.request.id
                );
                self.db
                    .set_order_status(&order.id(), OrderStatus::Skipped)
                    .await
                    .context("Failed to set order status to skipped")?;
            } else {
                tracing::info!("Request {:x} was locked by another prover but expired unfulfilled, setting status to pending proving", order.request.id);
                candidate_orders.push(order);
            }
        }

        // Fetch all the orders that we intend to lock and fulfill
        let pending_lock_orders = self
            .db
            .get_pending_lock_orders(current_block_timestamp + self.block_time)
            .await
            .context("Failed to find pending lock orders")?;

        tracing::trace!("Found orders that we intend to lock and fulfill: {pending_lock_orders:?}");

        for order in pending_lock_orders {
            let is_lock_expired = order.request.lock_expires_at() < current_block_timestamp;
            if is_lock_expired {
                tracing::info!("Request {:x} was scheduled to be locked by us, but its lock has now expired. Skipping.", order.request.id);
                self.db
                    .set_order_status(&order.id(), OrderStatus::Skipped)
                    .await
                    .context("Failed to set order status to skipped")?;
            } else if let Some((locker, _)) =
                self.db.get_request_locked(U256::from(order.request.id)).await?
            {
                let our_address = self.provider.default_signer_address().to_string().to_lowercase();
                let locker_address = locker.to_lowercase();
                if locker_address != our_address {
                    tracing::info!("Request {:x} was scheduled to be locked by us ({}), but is already locked by another prover ({}). Skipping.", order.request.id, our_address, locker_address);
                    self.db
                        .set_order_status(&order.id(), OrderStatus::Skipped)
                        .await
                        .context("Failed to set order status to skipped")?;
                } else {
                    // Edge case where we locked the order, but due to some reason was not moved to proving state. Should not happen.
                    tracing::info!("Request {:x} was scheduled to be locked by us, but is already locked by us. Proceeding to prove.", order.request.id);
                    candidate_orders.push(order);
                }
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

        let mut final_orders: Vec<Order> = Vec::new();
        for order in candidate_orders {
            let now = now_timestamp();
            if order.request.expires_at() < current_block_timestamp {
                tracing::debug!("Request {:x} has now expired. Skipping.", order.request.id);
                self.db
                    .set_order_status(&order.id(), OrderStatus::Skipped)
                    .await
                    .context("Failed to set order status to skipped")?;
            } else if order.request.expires_at() - now < min_deadline {
                tracing::debug!("Request {:x} deadline at {} is less than the minimum deadline {} seconds required to prove an order. Skipping.", order.request.id, order.request.expires_at(), min_deadline);
                self.db
                    .set_order_status(&order.id(), OrderStatus::Skipped)
                    .await
                    .context("Failed to set order status to skipped")?;
            } else {
                final_orders.push(order);
            }
        }

        tracing::info!(
            "After filtering invalid orders, found total of {} valid orders to proceed to locking and/or proving", 
            final_orders.len()
        );
        tracing::debug!(
            "Final orders ready for locking and/or proving after filtering: {final_orders:?}"
        );

        Ok(final_orders)
    }

    fn prioritize_orders(&self, orders: Vec<Order>) -> Vec<Order> {
        // Sort orders by priority - for lock and fulfill orders, use lock expiration, for fulfill after lock expire, use request expiration
        let mut sorted_orders = orders;
        sorted_orders.sort_by(|order_1, order_2| {
            let time1 = if order_1.fulfillment_type == FulfillmentType::LockAndFulfill {
                order_1.request.lock_expires_at()
            } else {
                order_1.request.expires_at()
            };
            let time2 = if order_2.fulfillment_type == FulfillmentType::LockAndFulfill {
                order_2.request.lock_expires_at()
            } else {
                order_2.request.expires_at()
            };
            time1.cmp(&time2)
        });

        tracing::debug!(
            "Orders ready for proving, prioritized. Before applying capacity limits: {:?}",
            sorted_orders
                .iter()
                .map(|order| format!(
                    "{} [Lock expires at: {}, Expires at: {}]",
                    order.id(),
                    order.request.lock_expires_at(),
                    order.request.expires_at()
                ))
                .collect::<Vec<_>>()
        );

        sorted_orders
    }

    async fn process_lock_and_fulfill_orders(
        &self,
        current_block: u64,
        orders: &[Order],
    ) -> Result<()> {
        self.lock_orders(current_block, orders.to_vec())
            .await
            .context("Failed to start locking orders")?;
        Ok(())
    }

    async fn process_fulfill_after_lock_expire_orders(&self, orders: &[Order]) -> Result<()> {
        self.prove_orders(orders.to_vec()).await.context("Failed to start proving orders")?;
        Ok(())
    }

    async fn apply_capacity_limits(
        &mut self,
        orders: Vec<Order>,
        max_concurrent_proofs: Option<u32>,
        peak_prove_khz: Option<u64>,
    ) -> Result<Vec<Order>> {
        let num_orders = orders.len();
        // Get our current capacity for proving orders given our config and the number of orders that are currently committed to be proven + fulfilled.
        let capacity = self.get_proving_order_capacity(max_concurrent_proofs).await?;
        let capacity_granted = capacity
            .request_capacity(num_orders.try_into().expect("Failed to convert order count to u32"));

        tracing::info!(
            "Current number of orders ready for proving: {}. Total capacity available based on max_concurrent_proofs: {capacity:?}, Capacity granted this iteration: {capacity_granted:?}",
            num_orders
        );

        // Given our capacity, truncate the order list
        let mut orders_truncated = orders;
        if orders_truncated.len() > capacity_granted as usize {
            orders_truncated.truncate(capacity_granted as usize);
        }

        let mut final_orders: Vec<Order> = Vec::new();

        // Apply peak khz limit if specified
        if let Some(peak_prove_khz) = peak_prove_khz {
            let mut commited_orders = self.db.get_committed_orders().await?;
            let num_commited_orders = commited_orders.len();
            let total_commited_cycles =
                commited_orders.iter().map(|order| order.total_cycles.unwrap()).sum::<u64>();

            // Estimate the time the prover will be available given our current committed orders.
            commited_orders.sort_by_key(|order| order.proving_started_at);
            let started_proving_at = if !commited_orders.is_empty() {
                commited_orders[0].proving_started_at.unwrap()
            } else {
                now_timestamp()
            };

            let proof_time_seconds = total_commited_cycles.div_ceil(1_000).div_ceil(peak_prove_khz);
            let mut prover_available_at = started_proving_at + proof_time_seconds;
            tracing::debug!("Already committed to {} orders, with a total cycle count of {}, a peak khz limit of {}, started working on them at {}, we estimate the prover will be available at {}", num_commited_orders, total_commited_cycles, peak_prove_khz, started_proving_at, prover_available_at);

            // For each order in consideration, check if it can be completed before its expiration.
            for order in orders_truncated {
                if order.total_cycles.is_none() {
                    tracing::warn!("Order {:x} has no total cycles, preflight was skipped? Not considering for peak khz limit", order.request.id);
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

                tracing::debug!("Order {} estimated to take {} seconds, and would be completed at {}. It expires at {}", order.id(), proof_time_seconds, completion_time, expiration);

                if completion_time > expiration {
                    tracing::info!("Order {:x} cannot be completed before its expiration at {}, proof estimated to complete at {}. Skipping", order.request.id, expiration, completion_time);
                    self.db
                        .set_order_status(&order.id(), OrderStatus::Skipped)
                        .await
                        .context("Failed to set order status to skipped")?;
                    continue;
                }

                final_orders.push(order);
                prover_available_at = completion_time;
            }
        } else {
            final_orders = orders_truncated;
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

    fn categorize_orders(&self, orders: Vec<Order>) -> OrdersByFulfillmentType {
        OrdersByFulfillmentType {
            lock_and_prove_orders: orders
                .iter()
                .filter(|order| order.fulfillment_type == FulfillmentType::LockAndFulfill)
                .cloned()
                .collect(),
            prove_orders: orders
                .iter()
                .filter(|order| order.fulfillment_type == FulfillmentType::FulfillAfterLockExpire)
                .cloned()
                .collect(),
        }
    }

    pub async fn start_monitor(mut self) -> Result<()> {
        self.back_scan_locks().await?;

        let mut last_block = 0;
        let mut first_block = 0;

        loop {
            let current_block = self.chain_monitor.current_block_number().await?;
            let current_block_timestamp = self.chain_monitor.current_block_timestamp().await?;
            if current_block != last_block {
                last_block = current_block;
                if first_block == 0 {
                    first_block = current_block;
                }
                tracing::trace!("Order monitor processing block {current_block} at timestamp {current_block_timestamp}");

                let (min_deadline, peak_prove_khz, max_concurrent_proofs) = {
                    let config = self.config.lock_all().context("Failed to read config")?;
                    (
                        config.market.min_deadline,
                        config.market.peak_prove_khz,
                        config.market.max_concurrent_proofs,
                    )
                };

                // Get orders that are valid and ready for locking/proving, skipping orders that are now invalid for proving, due to expiring, being locked by another prover, etc.
                let valid_orders =
                    self.get_valid_orders(current_block_timestamp, min_deadline).await?;

                if valid_orders.is_empty() {
                    tracing::trace!(
                        "No orders to lock and/or prove as of block timestamp {}",
                        current_block_timestamp
                    );
                    continue;
                }

                // Prioritize the orders that intend to fulfill based on when they need to locked and/or proven.
                let prioritized_orders = self.prioritize_orders(valid_orders);

                // Filter down the orders given our capacity and peak khz limits.
                let final_orders = self
                    .apply_capacity_limits(
                        prioritized_orders,
                        max_concurrent_proofs,
                        peak_prove_khz,
                    )
                    .await?;

                // Categorize orders by fulfillment type
                let categorized_orders = self.categorize_orders(final_orders);

                tracing::info!("After processing block {}[timestamp {}], we will now start locking and/or proving {} orders: {:?}", 
                    current_block,
                    current_block_timestamp,
                    categorized_orders.len(),
                    categorized_orders
                );

                // Proceed with orders based on their fulfillment type.
                // We first process fulfill after lock expire orders, as they are not dependent on sending a lock transaction, and can be kicked off for proving immediately.
                self.process_fulfill_after_lock_expire_orders(&categorized_orders.prove_orders)
                    .await?;
                // We then process lock and fulfill orders, they may take longer to kick off proving as we confirm the lock transactions.
                self.process_lock_and_fulfill_orders(
                    current_block,
                    &categorized_orders.lock_and_prove_orders,
                )
                .await?;
            }

            // Attempt to wait 1/2 a block time to catch each new block
            tokio::time::sleep(tokio::time::Duration::from_secs(self.block_time / 2)).await;
        }
    }
}

impl<P> RetryTask for OrderMonitor<P>
where
    P: Provider<Ethereum> + WalletProvider + 'static + Clone,
{
    fn spawn(&self) -> RetryRes {
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
    use crate::{db::SqliteDb, now_timestamp, FulfillmentType};
    use alloy::{
        network::EthereumWallet,
        node_bindings::Anvil,
        primitives::{Address, U256},
        providers::{ext::AnvilApi, ProviderBuilder},
        signers::local::PrivateKeySigner,
    };
    use boundless_market::contracts::{
        Input, InputType, Offer, Predicate, PredicateType, ProofRequest, RequestId, Requirements,
    };
    use boundless_market_test_utils::{deploy_boundless_market, deploy_hit_points};
    use chrono::Utc;
    use guest_assessor::{ASSESSOR_GUEST_ID, ASSESSOR_GUEST_PATH};
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

    async fn setup_test() -> (OrderMonitor<TestProvider>, DbObj, Address, ConfigLock) {
        let anvil = Anvil::new().spawn();
        let signer: PrivateKeySigner = anvil.keys()[0].clone().into();
        let provider = Arc::new(
            ProviderBuilder::new()
                .wallet(EthereumWallet::from(signer.clone()))
                .connect(&anvil.endpoint())
                .await
                .unwrap(),
        );

        let market_address = Address::ZERO;
        let db: DbObj = Arc::new(SqliteDb::new("sqlite::memory:").await.unwrap());
        let config = ConfigLock::default();
        let block_time = 2;

        let chain_monitor = Arc::new(ChainMonitorService::new(provider.clone()).await.unwrap());
        let monitor = OrderMonitor::new(
            db.clone(),
            provider.clone(),
            chain_monitor.clone(),
            config.clone(),
            block_time,
            market_address,
        )
        .unwrap();

        (monitor, db, market_address, config)
    }

    fn create_test_order(
        market_address: Address,
        chain_id: u64,
        fulfillment_type: FulfillmentType,
        bidding_start: u64,
        lock_timeout: u64,
        timeout: u64,
    ) -> Order {
        let request = ProofRequest::new(
            RequestId::new(Address::ZERO, 1),
            Requirements::new(
                Digest::ZERO,
                Predicate { predicateType: PredicateType::PrefixMatch, data: Default::default() },
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

        Order {
            status: OrderStatus::WaitingToLock,
            updated_at: Utc::now(),
            target_timestamp: Some(0),
            request,
            image_id: None,
            input_id: None,
            proof_id: None,
            compressed_proof_id: None,
            expire_timestamp: None,
            client_sig: vec![0; 65].into(),
            lock_price: None,
            fulfillment_type,
            error_msg: None,
            boundless_market_address: market_address,
            chain_id,
            total_cycles: None,
            proving_started_at: None,
        }
    }

    // Original tests
    #[tokio::test]
    #[traced_test]
    async fn back_scan_lock() {
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
            Address::ZERO,
            hit_points,
            Digest::from(ASSESSOR_GUEST_ID),
            format!("file://{ASSESSOR_GUEST_PATH}"),
            Some(signer.address()),
        )
        .await
        .unwrap();
        let boundless_market = BoundlessMarketService::new(
            market_address,
            provider.clone(),
            provider.default_signer_address(),
        );

        let db: DbObj = Arc::new(SqliteDb::new("sqlite::memory:").await.unwrap());
        let config = ConfigLock::default();

        let block_time = 2;
        let min_price = 1;
        let max_price = 2;

        let request = ProofRequest::new(
            RequestId::new(signer.address(), 1),
            Requirements::new(
                Digest::ZERO,
                Predicate { predicateType: PredicateType::PrefixMatch, data: Default::default() },
            ),
            "http://risczero.com/image",
            Input { inputType: InputType::Inline, data: Default::default() },
            Offer {
                minPrice: U256::from(min_price),
                maxPrice: U256::from(max_price),
                biddingStart: now_timestamp(),
                rampUpPeriod: 1,
                timeout: 100,
                lockTimeout: 100,
                lockStake: U256::from(0),
            },
        );
        tracing::info!("addr: {} ID: {:x}", signer.address(), request.id);

        let chain_id = provider.get_chain_id().await.unwrap();
        let client_sig =
            request.sign_request(&signer, market_address, chain_id).await.unwrap().as_bytes();

        let order = Order {
            status: OrderStatus::WaitingToLock,
            updated_at: Utc::now(),
            target_timestamp: Some(0),
            request,
            image_id: None,
            input_id: None,
            proof_id: None,
            compressed_proof_id: None,
            expire_timestamp: None,
            client_sig: client_sig.into(),
            lock_price: None,
            fulfillment_type: FulfillmentType::LockAndFulfill,
            error_msg: None,
            boundless_market_address: market_address,
            chain_id,
            total_cycles: None,
            proving_started_at: None,
        };
        let request_id = boundless_market.submit_request(&order.request, &signer).await.unwrap();
        assert!(order.id().contains(&format!("{:x}", request_id)));

        provider.anvil_mine(Some(2), Some(block_time)).await.unwrap();

        db.add_order(order.clone()).await.unwrap();
        db.set_last_block(1).await.unwrap();

        let chain_monitor = Arc::new(ChainMonitorService::new(provider.clone()).await.unwrap());
        tokio::spawn(chain_monitor.spawn());
        let monitor = OrderMonitor::new(
            db.clone(),
            provider.clone(),
            chain_monitor.clone(),
            config.clone(),
            block_time,
            market_address,
        )
        .unwrap();

        let orders = monitor.back_scan_locks().await.unwrap();
        assert_eq!(orders, 1);

        let order = db.get_order(&order.id()).await.unwrap().unwrap();
        if let OrderStatus::Failed = order.status {
            let err = order.error_msg.expect("Missing error message for failed order");
            panic!("order failed: {err}");
        }
        assert!(matches!(order.status, OrderStatus::PendingProving));
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
            Address::ZERO,
            hit_points,
            Digest::from(ASSESSOR_GUEST_ID),
            format!("file://{ASSESSOR_GUEST_PATH}"),
            Some(signer.address()),
        )
        .await
        .unwrap();
        let boundless_market = BoundlessMarketService::new(
            market_address,
            provider.clone(),
            provider.default_signer_address(),
        );

        let db: DbObj = Arc::new(SqliteDb::new("sqlite::memory:").await.unwrap());
        let config = ConfigLock::default();

        let block_time = 2;
        let min_price = 1;
        let max_price = 2;

        let request = ProofRequest::new(
            RequestId::new(signer.address(), boundless_market.index_from_nonce().await.unwrap()),
            Requirements::new(
                Digest::ZERO,
                Predicate { predicateType: PredicateType::PrefixMatch, data: Default::default() },
            ),
            "http://risczero.com/image",
            Input { inputType: InputType::Inline, data: Default::default() },
            Offer {
                minPrice: U256::from(min_price),
                maxPrice: U256::from(max_price),
                biddingStart: now_timestamp(),
                rampUpPeriod: 1,
                timeout: 100,
                lockTimeout: 100,
                lockStake: U256::from(0),
            },
        );
        tracing::info!("addr: {} ID: {:x}", signer.address(), request.id);

        let chain_id = provider.get_chain_id().await.unwrap();
        let client_sig = request
            .sign_request(&signer, market_address, chain_id)
            .await
            .unwrap()
            .as_bytes()
            .into();
        let order = Order {
            status: OrderStatus::WaitingToLock,
            updated_at: Utc::now(),
            target_timestamp: Some(0),
            request,
            image_id: None,
            input_id: None,
            proof_id: None,
            compressed_proof_id: None,
            expire_timestamp: None,
            client_sig,
            lock_price: None,
            fulfillment_type: FulfillmentType::LockAndFulfill,
            error_msg: None,
            boundless_market_address: market_address,
            chain_id,
            total_cycles: None,
            proving_started_at: None,
        };

        let _request_id = boundless_market.submit_request(&order.request, &signer).await.unwrap();

        db.add_order(order.clone()).await.unwrap();

        db.set_last_block(0).await.unwrap();

        let chain_monitor = Arc::new(ChainMonitorService::new(provider.clone()).await.unwrap());
        tokio::spawn(chain_monitor.spawn());
        let monitor = OrderMonitor::new(
            db.clone(),
            provider.clone(),
            chain_monitor.clone(),
            config.clone(),
            block_time,
            market_address,
        )
        .unwrap();

        run_with_monitor(monitor, async move {
            // loop for 20 seconds
            for _ in 0..20 {
                let order = db.get_order(&order.id()).await.unwrap().unwrap();
                if order.status == OrderStatus::PendingProving {
                    break;
                }
                tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
            }

            let order = db.get_order(&order.id()).await.unwrap().unwrap();
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
        let (monitor, db, market_address, _) = setup_test().await;
        let current_timestamp = now_timestamp();
        let chain_id = 1;

        // Create an expired order
        let expired_order = create_test_order(
            market_address,
            chain_id,
            FulfillmentType::LockAndFulfill,
            current_timestamp - 100,
            50,
            50,
        );
        db.add_order(expired_order.clone()).await.unwrap();

        let result = monitor.get_valid_orders(current_timestamp, 0).await.unwrap();

        assert!(result.is_empty());

        let order = db.get_order(&expired_order.id()).await.unwrap().unwrap();
        assert_eq!(order.status, OrderStatus::Skipped);
    }

    #[tokio::test]
    #[traced_test]
    async fn test_filter_insufficient_deadline() {
        let (monitor, db, market_address, _) = setup_test().await;
        let current_timestamp = now_timestamp();
        let chain_id = 1;

        // Create an order with insufficient deadline
        let order = create_test_order(
            market_address,
            chain_id,
            FulfillmentType::LockAndFulfill,
            current_timestamp,
            45,
            45,
        );
        db.add_order(order.clone()).await.unwrap();
        // Create an order with insufficient deadline
        let order = create_test_order(
            market_address,
            chain_id,
            FulfillmentType::FulfillAfterLockExpire,
            current_timestamp,
            1,
            45,
        );
        db.add_order(order.clone()).await.unwrap();

        let result = monitor.get_valid_orders(current_timestamp, 100).await.unwrap();

        assert!(result.is_empty());

        let order = db.get_order(&order.id()).await.unwrap().unwrap();
        assert_eq!(order.status, OrderStatus::Skipped);
    }

    #[tokio::test]
    async fn test_filter_locked_by_others() {
        let (monitor, db, market_address, _) = setup_test().await;
        let current_timestamp = now_timestamp();
        let chain_id = 1;

        // Create an order that's locked by another prover
        let order = create_test_order(
            market_address,
            chain_id,
            FulfillmentType::LockAndFulfill,
            current_timestamp,
            100,
            200,
        );
        db.add_order(order.clone()).await.unwrap();
        db.set_request_locked(
            U256::from(order.request.id),
            &Address::ZERO.to_string(),
            current_timestamp,
        )
        .await
        .unwrap();

        let result =
            monitor.get_valid_orders(current_timestamp, current_timestamp + 100).await.unwrap();

        assert!(result.is_empty());

        let order = db.get_order(&order.id()).await.unwrap().unwrap();
        assert_eq!(order.status, OrderStatus::Skipped);
    }

    // Sorting tests
    #[tokio::test]
    async fn test_prioritize_orders() {
        let (monitor, _, market_address, _) = setup_test().await;
        let current_timestamp = now_timestamp();
        let chain_id = 1;

        // Create orders with different expiration times
        // Must lock and fulfill within 50 seconds
        let order1 = create_test_order(
            market_address,
            chain_id,
            FulfillmentType::LockAndFulfill,
            current_timestamp,
            50,
            200,
        );
        // Must lock and fulfill within 100 seconds.
        let order2 = create_test_order(
            market_address,
            chain_id,
            FulfillmentType::LockAndFulfill,
            current_timestamp,
            100,
            200,
        );
        // Must fulfill after lock expires within 51 seconds.
        let order3 = create_test_order(
            market_address,
            chain_id,
            FulfillmentType::FulfillAfterLockExpire,
            current_timestamp,
            1,
            51,
        );
        // Must fulfill after lock expires within 53 seconds.
        let order4 = create_test_order(
            market_address,
            chain_id,
            FulfillmentType::FulfillAfterLockExpire,
            current_timestamp,
            1,
            53,
        );

        let result = monitor.prioritize_orders(vec![
            order1.clone(),
            order2.clone(),
            order3.clone(),
            order4.clone(),
        ]);

        assert!(result[0].id() == order1.id());
        assert!(result[1].id() == order3.id());
        assert!(result[2].id() == order4.id());
        assert!(result[3].id() == order2.id());
    }

    // Processing tests
    #[tokio::test]
    #[traced_test]
    async fn test_process_lock_and_fulfill_orders() {
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
            Address::ZERO,
            hit_points,
            Digest::from(ASSESSOR_GUEST_ID),
            format!("file://{ASSESSOR_GUEST_PATH}"),
            Some(signer.address()),
        )
        .await
        .unwrap();
        let boundless_market = BoundlessMarketService::new(
            market_address,
            provider.clone(),
            provider.default_signer_address(),
        );

        let db: DbObj = Arc::new(SqliteDb::new("sqlite::memory:").await.unwrap());
        let config = ConfigLock::default();

        let block_time = 2;
        let min_price = 1;
        let max_price = 2;

        let request = ProofRequest::new(
            RequestId::new(signer.address(), boundless_market.index_from_nonce().await.unwrap()),
            Requirements::new(
                Digest::ZERO,
                Predicate { predicateType: PredicateType::PrefixMatch, data: Default::default() },
            ),
            "http://risczero.com/image",
            Input { inputType: InputType::Inline, data: Default::default() },
            Offer {
                minPrice: U256::from(min_price),
                maxPrice: U256::from(max_price),
                biddingStart: now_timestamp(),
                rampUpPeriod: 1,
                timeout: 500,
                lockTimeout: 500,
                lockStake: U256::from(0),
            },
        );
        tracing::info!("addr: {} ID: {:x}", signer.address(), request.id);

        let chain_id = provider.get_chain_id().await.unwrap();
        let client_sig = request
            .sign_request(&signer, market_address, chain_id)
            .await
            .unwrap()
            .as_bytes()
            .into();
        let order = Order {
            status: OrderStatus::WaitingToLock,
            updated_at: Utc::now(),
            target_timestamp: Some(0),
            request,
            image_id: None,
            input_id: None,
            proof_id: None,
            compressed_proof_id: None,
            expire_timestamp: None,
            client_sig,
            lock_price: None,
            fulfillment_type: FulfillmentType::LockAndFulfill,
            error_msg: None,
            boundless_market_address: market_address,
            chain_id,
            total_cycles: None,
            proving_started_at: None,
        };

        let _request_id = boundless_market.submit_request(&order.request, &signer).await.unwrap();

        db.add_order(order.clone()).await.unwrap();

        let chain_monitor = Arc::new(ChainMonitorService::new(provider.clone()).await.unwrap());
        tokio::spawn(chain_monitor.spawn());
        let monitor = OrderMonitor::new(
            db.clone(),
            provider.clone(),
            chain_monitor.clone(),
            config.clone(),
            block_time,
            market_address,
        )
        .unwrap();

        run_with_monitor(monitor, async move {
            // loop for 20 seconds
            for _ in 0..20 {
                let order = db.get_order(&order.id()).await.unwrap().unwrap();
                if order.status == OrderStatus::PendingProving {
                    break;
                }
                tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
            }

            let order = db.get_order(&order.id()).await.unwrap().unwrap();
            assert_eq!(order.status, OrderStatus::PendingProving);
        })
        .await;
    }

    #[tokio::test]
    async fn test_process_fulfill_after_lock_expire_orders() {
        let (monitor, db, market_address, _) = setup_test().await;
        let current_timestamp = now_timestamp();
        let chain_id = 1;

        let order = create_test_order(
            market_address,
            chain_id,
            FulfillmentType::FulfillAfterLockExpire,
            current_timestamp,
            100,
            200,
        );
        db.add_order(order.clone()).await.unwrap();

        monitor.process_fulfill_after_lock_expire_orders(&[order.clone()]).await.unwrap();

        let updated_order = db.get_order(&order.id()).await.unwrap().unwrap();
        assert_eq!(updated_order.status, OrderStatus::PendingProving);
    }

    #[tokio::test]
    async fn test_apply_capacity_limits_unlimited() {
        let (mut monitor, _, market_address, _) = setup_test().await;
        let current_timestamp = now_timestamp();
        let chain_id = 1;

        // Create multiple orders
        let orders: Vec<Order> = (0..5)
            .map(|_i| {
                create_test_order(
                    market_address,
                    chain_id,
                    FulfillmentType::LockAndFulfill,
                    current_timestamp,
                    100,
                    200,
                )
            })
            .collect();

        // Test with unlimited capacity
        let result = monitor.apply_capacity_limits(orders.clone(), None, None).await.unwrap();

        // Should return all orders since capacity is unlimited
        assert_eq!(result.len(), orders.len());
    }

    #[tokio::test]
    async fn test_apply_capacity_limits_proving() {
        let (mut monitor, db, market_address, _) = setup_test().await;
        let current_timestamp = now_timestamp();
        let chain_id = 1;

        // Create multiple orders
        let orders: Vec<Order> = (0..5)
            .map(|_i| {
                create_test_order(
                    market_address,
                    chain_id,
                    FulfillmentType::LockAndFulfill,
                    current_timestamp,
                    100,
                    200,
                )
            })
            .collect();

        // Add a committed order to simulate existing workload
        let mut committed_order = create_test_order(
            market_address,
            chain_id,
            FulfillmentType::LockAndFulfill,
            current_timestamp,
            100,
            200,
        );
        committed_order.status = OrderStatus::Proving;
        committed_order.proving_started_at = Some(current_timestamp);
        db.add_order(committed_order).await.unwrap();

        // Test with limited capacity (3 slots)
        let result = monitor.apply_capacity_limits(orders.clone(), Some(3), None).await.unwrap();

        // Should return only 2 orders due to concurrent proving capacity limit of 3, with 1 order already committed.
        assert_eq!(result.len(), 2);
    }

    #[tokio::test]
    async fn test_apply_capacity_limits_committed_work_too_large() {
        let (mut monitor, db, market_address, _) = setup_test().await;
        let current_timestamp = now_timestamp();
        let chain_id = 1;

        // Create two orders with low cycle counts. We do not commence proving as we are still working on a large job.
        let mut orders = Vec::new();

        // Order 1: 1000 cycles
        let mut order1 = create_test_order(
            market_address,
            chain_id,
            FulfillmentType::LockAndFulfill,
            current_timestamp,
            100,
            200,
        );
        order1.total_cycles = Some(1000);
        orders.push(order1);

        // Order 2: 2000 cycles
        let mut order2 = create_test_order(
            market_address,
            chain_id,
            FulfillmentType::LockAndFulfill,
            current_timestamp,
            100,
            200,
        );
        order2.total_cycles = Some(2000);
        orders.push(order2);

        // Add a large committed order to simulate existing workload
        let mut committed_order = create_test_order(
            market_address,
            chain_id,
            FulfillmentType::LockAndFulfill,
            current_timestamp,
            100,
            200,
        );
        committed_order.status = OrderStatus::Proving;
        committed_order.total_cycles = Some(10000000000000000);
        committed_order.proving_started_at = Some(current_timestamp);
        db.add_order(committed_order).await.unwrap();

        let result = monitor.apply_capacity_limits(orders.clone(), None, Some(1)).await.unwrap();

        assert!(result.is_empty());
    }

    #[tokio::test]
    async fn test_apply_capacity_limits_skip_proof_time_past_expiration() {
        let (mut monitor, db, market_address, _) = setup_test().await;
        let current_timestamp = now_timestamp();
        let chain_id = 1;

        // Create orders with different expiration times
        let mut orders = Vec::new();

        // Order 1: Will expire soon
        let mut order1 = create_test_order(
            market_address,
            chain_id,
            FulfillmentType::LockAndFulfill,
            current_timestamp,
            5,
            5,
        );
        order1.total_cycles = Some(1000000000000);
        orders.push(order1.clone());
        db.add_order(order1).await.unwrap();
        // Order 2: Longer expiration
        let mut order2 = create_test_order(
            market_address,
            chain_id,
            FulfillmentType::LockAndFulfill,
            current_timestamp,
            100,
            200,
        );
        order2.total_cycles = Some(2000);
        orders.push(order2.clone());
        db.add_order(order2).await.unwrap();

        // Test with peak khz limit
        let result = monitor.apply_capacity_limits(orders.clone(), None, Some(1)).await.unwrap();

        // Should skip the order that would expire before completion
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].request.id, orders[1].request.id);
        assert_eq!(
            db.get_order(&orders[0].id()).await.unwrap().unwrap().status,
            OrderStatus::Skipped
        );
    }
}
