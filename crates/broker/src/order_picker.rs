// Copyright (c) 2025 RISC Zero, Inc.
//
// All rights reserved.

use std::sync::Arc;

use crate::{
    chain_monitor::ChainMonitorService,
    config::ConfigLock,
    db::DbObj,
    provers::{ProverError, ProverObj},
    task::{RetryRes, RetryTask, SupervisorErr},
    FulfillmentType, Order,
};
use crate::{now_timestamp, provers::ProofResult};
use alloy::{
    network::Ethereum,
    primitives::{
        aliases::U96,
        utils::{format_ether, format_units, parse_ether},
        Address, U256,
    },
    providers::{Provider, WalletProvider},
};
use anyhow::{Context, Result};
use boundless_market::{
    contracts::{boundless_market::BoundlessMarketService, RequestError},
    selector::{ProofType, SupportedSelectors},
};
use thiserror::Error;
use tokio::task::JoinSet;

use OrderPricingOutcome::{Lock, ProveAfterLockExpire, Skip};

/// Maximum number of orders to concurrently work on pricing. Used to limit pricing tasks spawned.
const MAX_PRICING_BATCH_SIZE: u32 = 10;

/// Gas allocated to verifying a smart contract signature. Copied from BoundlessMarket.sol.
const ERC1271_MAX_GAS_FOR_CHECK: u64 = 100000;

#[derive(Error, Debug)]
#[non_exhaustive]
pub enum PriceOrderErr {
    #[error("failed to fetch / push input: {0}")]
    FetchInputErr(#[source] anyhow::Error),

    #[error("failed to fetch / push image: {0}")]
    FetchImageErr(#[source] anyhow::Error),

    #[error("guest panicked: {0}")]
    GuestPanic(String),

    #[error("invalid request")]
    RequestError(#[from] RequestError),

    #[error(transparent)]
    OtherErr(#[from] anyhow::Error),
}

#[derive(Clone)]
pub struct OrderPicker<P> {
    db: DbObj,
    config: ConfigLock,
    prover: ProverObj,
    provider: Arc<P>,
    chain_monitor: Arc<ChainMonitorService<P>>,
    market: BoundlessMarketService<Arc<P>>,
    supported_selectors: SupportedSelectors,
}

#[derive(Debug)]
#[non_exhaustive]
enum OrderPricingOutcome {
    // Order should be locked and proving commence after lock is secured
    Lock {
        total_cycles: Option<u64>,
        target_timestamp_secs: u64,
        // TODO handle checking what time the lock should occur before, when estimating proving time.
        expiry_secs: u64,
    },
    // Do not lock the order, but consider proving and fulfilling it after the lock expires
    ProveAfterLockExpire {
        total_cycles: Option<u64>,
        lock_expire_timestamp_secs: u64,
        expiry_secs: u64,
    },
    // Do not accept engage order
    Skip,
}

impl<P> OrderPicker<P>
where
    P: Provider<Ethereum> + 'static + Clone + WalletProvider,
{
    pub fn new(
        db: DbObj,
        config: ConfigLock,
        prover: ProverObj,
        market_addr: Address,
        provider: Arc<P>,
        chain_monitor: Arc<ChainMonitorService<P>>,
    ) -> Self {
        let market = BoundlessMarketService::new(
            market_addr,
            provider.clone(),
            provider.default_signer_address(),
        );
        Self {
            db,
            config,
            prover,
            provider,
            chain_monitor,
            market,
            supported_selectors: SupportedSelectors::default(),
        }
    }

    async fn price_order_and_update_db(&self, order: &Order) -> bool {
        let f = || async {
            let request_id = order.request.id;
            match self.price_order(order).await {
                Ok(Lock { total_cycles, target_timestamp_secs, expiry_secs }) => {
                    tracing::info!("Setting order with request id {request_id:x} to lock at {target_timestamp_secs}");
                    self.db
                        .set_order_lock(
                            &order.id(),
                            target_timestamp_secs,
                            expiry_secs,
                            total_cycles,
                        )
                        .await
                        .context("Failed to set_order_lock")?;
                    Ok::<_, PriceOrderErr>(true)
                }
                Ok(ProveAfterLockExpire {
                    total_cycles,
                    lock_expire_timestamp_secs,
                    expiry_secs,
                }) => {
                    tracing::info!("Setting order with request id {request_id:x} to prove after lock expiry at {lock_expire_timestamp_secs}");
                    self.db
                        .set_order_fulfill_after_lock_expire(
                            &order.id(),
                            lock_expire_timestamp_secs,
                            expiry_secs,
                            total_cycles,
                        )
                        .await
                        .context("Failed to set_order_fulfill_after_lock_expire")?;
                    Ok(true)
                }
                Ok(Skip) => {
                    tracing::info!("Skipping order with request id {request_id:x}");
                    self.db.skip_order(&order.id()).await.context("Failed to delete order")?;
                    Ok(false)
                }
                Err(err) => {
                    tracing::error!("Failed to price order with request id {request_id:x}: {err}");
                    self.db
                        .set_order_failure(&order.id(), err.to_string())
                        .await
                        .context("Failed to set_order_failure")?;
                    Ok(false)
                }
            }
        };

        match f().await {
            Ok(true) => true,
            Ok(false) => false,
            Err(err) => {
                tracing::error!("Failed to update db for order {}: {err}", order.id());
                false
            }
        }
    }

    async fn price_order(&self, order: &Order) -> Result<OrderPricingOutcome, PriceOrderErr> {
        let order_id = order.id();
        let request_id = order.request.id;
        tracing::debug!("Processing order {order_id} with request id {request_id:x}: {order:?}");

        let (min_deadline, allowed_addresses_opt) = {
            let config = self.config.lock_all().context("Failed to read config")?;
            (config.market.min_deadline, config.market.allow_client_addresses.clone())
        };

        // Initial sanity checks:
        if let Some(allow_addresses) = allowed_addresses_opt {
            let client_addr = order.request.client_address();
            if !allow_addresses.contains(&client_addr) {
                tracing::info!("Removing order with request id {request_id:x} from {client_addr} because it is not in allowed addrs");
                return Ok(Skip);
            }
        }

        if !self.supported_selectors.is_supported(order.request.requirements.selector) {
            tracing::info!(
                "Removing order with request id {request_id:x} because it has an unsupported selector requirement"
            );

            return Ok(Skip);
        };

        // Lock expiration is the timestamp before which the order must be filled in order to avoid slashing
        let lock_expiration =
            order.request.offer.biddingStart + order.request.offer.lockTimeout as u64;
        // order expiration is the timestamp after which the order can no longer be filled by anyone.
        let order_expiration =
            order.request.offer.biddingStart + order.request.offer.timeout as u64;

        let now = now_timestamp();

        // If order_expiration > lock_expiration the period in-between is when order can be filled
        // by anyone without staking to partially claim the slashed stake
        let lock_expired = order.fulfillment_type == FulfillmentType::FulfillAfterLockExpire;

        let (expiration, lockin_stake) = if lock_expired {
            (order_expiration, U256::ZERO)
        } else {
            (lock_expiration, U256::from(order.request.offer.lockStake))
        };

        if expiration <= now {
            tracing::info!("Removing order with request id {request_id:x} because it has expired");
            return Ok(Skip);
        };

        // Does the order expire within the min deadline
        let seconds_left = expiration.saturating_sub(now);
        if seconds_left <= min_deadline {
            tracing::info!("Removing order with request id {request_id:x} because it expires within min_deadline: {seconds_left}, min_deadline: {min_deadline}");
            return Ok(Skip);
        }

        // Check if the stake is sane and if we can afford it
        // For lock expired orders, we don't check the max stake because we can't lock those orders.
        let max_stake = {
            let config = self.config.lock_all().context("Failed to read config")?;
            parse_ether(&config.market.max_stake).context("Failed to parse max_stake")?
        };

        if !lock_expired && lockin_stake > max_stake {
            tracing::info!("Removing high stake order with request id {request_id:x}, lock stake: {lockin_stake}, max stake: {max_stake}");
            return Ok(Skip);
        }

        // Check that we have both enough staking tokens to stake, and enough gas tokens to lock and fulfil
        // NOTE: We use the current gas price and a rough heuristic on gas costs. Its possible that
        // gas prices may go up (or down) by the time its time to fulfill. This does not aim to be
        // a tight estimate, although improving this estimate will allow for a more profit.
        let gas_price =
            self.chain_monitor.current_gas_price().await.context("Failed to get gas price")?;
        let order_gas = if lock_expired {
            // No need to include lock gas if its a lock expired order
            U256::from(self.estimate_gas_to_fulfill(order).await?)
        } else {
            U256::from(
                self.estimate_gas_to_lock(order).await?
                    + self.estimate_gas_to_fulfill(order).await?,
            )
        };
        let order_gas_cost = U256::from(gas_price) * order_gas;
        let available_gas = self.available_gas_balance().await?;
        let available_stake = self.available_stake_balance().await?;
        tracing::debug!(
            "Estimated {order_gas} gas to {} request {request_id:x}; {} ether @ {} gwei",
            if lock_expired { "fulfill" } else { "lock and fulfill" },
            format_ether(order_gas_cost),
            format_units(gas_price, "gwei").unwrap()
        );

        if order_gas_cost > order.request.offer.maxPrice && !lock_expired {
            // Cannot check the gas cost for lock expired orders where the reward is a fraction of the stake
            // TODO: This can be added once we have a price feed for the stake token in gas tokens
            tracing::info!(
                "Estimated gas cost to lock and fulfill request {request_id:x}: {} exceeds max price; max price {}",
                format_ether(order_gas_cost),
                format_ether(order.request.offer.maxPrice)
            );
            self.db.skip_order(&order.id()).await.context("Failed to delete order")?;
            return Ok(Skip);
        }

        if order_gas_cost > available_gas {
            tracing::warn!("Estimated there will be insufficient gas for order {order_id} after locking and fulfilling pending orders; available_gas {} ether", format_ether(available_gas));
            self.db.skip_order(&order.id()).await.context("Failed to delete order")?;
            return Ok(Skip);
        }

        if !lock_expired && lockin_stake > available_stake {
            tracing::warn!(
                "Insufficient available stake to lock order {order_id}. Requires {lockin_stake}, has {available_stake}"
            );
            return Ok(Skip);
        }

        let (skip_preflight, max_mcycle_limit) = {
            let config = self.config.lock_all().context("Failed to read config")?;
            let skip_preflight =
                if let Some(skip_preflights) = config.market.skip_preflight_ids.as_ref() {
                    skip_preflights.contains(&order.request.requirements.imageId)
                } else {
                    false
                };

            (skip_preflight, config.market.max_mcycle_limit)
        };

        // If we skip preflight we lock the order asap, or schedule it to be proven after the lock expires asap
        if skip_preflight {
            if lock_expired {
                return Ok(ProveAfterLockExpire {
                    total_cycles: None,
                    lock_expire_timestamp_secs: lock_expiration,
                    expiry_secs: order_expiration,
                });
            } else {
                return Ok(Lock {
                    total_cycles: None,
                    target_timestamp_secs: 0,
                    expiry_secs: expiration,
                });
            }
        }

        // TODO: Move URI handling like this into the prover impls
        let image_id = crate::upload_image_uri(&self.prover, order, &self.config)
            .await
            .map_err(PriceOrderErr::FetchImageErr)?;

        let input_id = crate::upload_input_uri(&self.prover, order, &self.config)
            .await
            .map_err(PriceOrderErr::FetchInputErr)?;

        // Record the image/input IDs for proving stage
        self.db
            .set_image_input_ids(&order.id(), &image_id, &input_id)
            .await
            .context("Failed to record Input/Image IDs to DB")?;

        // Create a executor limit based on the max price of the order
        let mut exec_limit_cycles: u64 = if lock_expired {
            let min_mcycle_price_stake_token = {
                let config = self.config.lock_all().context("Failed to read config")?;
                parse_ether(&config.market.mcycle_price_stake_token)
                    .context("Failed to parse mcycle_price")?
            };

            if min_mcycle_price_stake_token == U256::ZERO {
                tracing::warn!("min_mcycle_price_stake_token is 0, setting unlimited exec limit");
                u64::MAX
            } else {
                // Note this does not account for gas cost unlike a normal order
                // TODO: Update to account for gas once the stake token to gas token exchange rate is known
                let price = order.request.offer.stake_reward_if_locked_and_not_fulfilled();
                let min_cycle_price_stake_token =
                    min_mcycle_price_stake_token.div_ceil(U256::from(1_000_000));
                (price / min_cycle_price_stake_token)
                    .try_into()
                    .context("Failed to convert U256 exec limit to u64")?
            }
        } else {
            let min_mcycle_price = {
                let config = self.config.lock_all().context("Failed to read config")?;
                parse_ether(&config.market.mcycle_price).context("Failed to parse mcycle_price")?
            };
            let min_cycle_price = min_mcycle_price.div_ceil(U256::from(1_000_000));
            (U256::from(order.request.offer.maxPrice).saturating_sub(order_gas_cost)
                / min_cycle_price)
                .try_into()
                .context("Failed to convert U256 exec limit to u64")?
        };

        if exec_limit_cycles == 0 {
            tracing::info!(
                "Removing order with request id {request_id:x} because it's exec limit is below 0 cycles"
            );

            return Ok(Skip);
        }

        // If a max_mcycle_limit is configured, override the exec limit if the order is over that limit
        if let Some(config_mcycle_limit) = max_mcycle_limit {
            let config_cycle_limit = config_mcycle_limit * 1_000_000;
            if exec_limit_cycles >= config_cycle_limit {
                tracing::info!("Order with request id {request_id:x} exec limit computed from max price exceeds config max_mcycle_limit, setting exec limit to max_mcycle_limit");
                exec_limit_cycles = config_cycle_limit;
            }
        }

        tracing::debug!(
            "Starting preflight execution of {request_id:x} exec limit {} cycles (~{} mcycles)",
            exec_limit_cycles,
            exec_limit_cycles / 1_000_000
        );
        // TODO add a future timeout here to put a upper bound on how long to preflight for
        let proof_res = match self
            .prover
            .preflight(
                &image_id,
                &input_id,
                vec![],
                /* TODO assumptions */ Some(exec_limit_cycles),
            )
            .await
        {
            Ok(res) => res,
            Err(err) => match err {
                ProverError::ProvingFailed(ref err_msg)
                    if err_msg.contains("Session limit exceeded") =>
                {
                    tracing::info!(
                        "Skipping order {request_id:x} due to session limit exceeded: {}",
                        err_msg
                    );
                    return Ok(Skip);
                }
                ProverError::ProvingFailed(ref err_msg) if err_msg.contains("GuestPanic") => {
                    return Err(PriceOrderErr::GuestPanic(err_msg.clone()));
                }
                _ => return Err(PriceOrderErr::OtherErr(err.into())),
            },
        };

        // If a max_mcycle_limit is configured check if the order is over that limit
        if let Some(mcycle_limit) = max_mcycle_limit {
            let mcycles = proof_res.stats.total_cycles / 1_000_000;
            if mcycles >= mcycle_limit {
                tracing::info!("Order with request id {request_id:x} max_mcycle_limit check failed req: {mcycle_limit} | config: {mcycles}");
                return Ok(Skip);
            }
        }

        let journal = self
            .prover
            .get_preflight_journal(&proof_res.id)
            .await
            .context("Failed to fetch preflight journal")?
            .context("Failed to find preflight journal")?;

        // ensure the journal is a size we are willing to submit on-chain
        let max_journal_bytes =
            self.config.lock_all().context("Failed to read config")?.market.max_journal_bytes;
        if journal.len() > max_journal_bytes {
            tracing::info!(
                "Order with request id {request_id:x} journal larger than set limit ({} > {}), skipping",
                journal.len(),
                max_journal_bytes
            );
            return Ok(Skip);
        }

        // Validate the predicates:
        if !order.request.requirements.predicate.eval(journal.clone()) {
            tracing::info!("Order with request id {request_id:x} predicate check failed, skipping");
            return Ok(Skip);
        }

        self.evaluate_order(order, &proof_res, order_gas_cost, lock_expired).await
    }

    async fn evaluate_order(
        &self,
        order: &Order,
        proof_res: &ProofResult,
        order_gas_cost: U256,
        lock_expired: bool,
    ) -> Result<OrderPricingOutcome, PriceOrderErr> {
        if lock_expired {
            return self.evaluate_lock_expired_order(order, proof_res).await;
        } else {
            self.evaluate_lockable_order(order, proof_res, order_gas_cost).await
        }
    }

    /// Evaluate if a regular lockable order is worth picking based on the price and the configured min mcycle price
    async fn evaluate_lockable_order(
        &self,
        order: &Order,
        proof_res: &ProofResult,
        order_gas_cost: U256,
    ) -> Result<OrderPricingOutcome, PriceOrderErr> {
        let config_min_mcycle_price = {
            let config = self.config.lock_all().context("Failed to read config")?;
            parse_ether(&config.market.mcycle_price).context("Failed to parse mcycle_price")?
        };

        let request_id = order.request.id;
        let one_mill = U256::from(1_000_000);

        let mcycle_price_min = (U256::from(order.request.offer.minPrice)
            .saturating_sub(order_gas_cost)
            / U256::from(proof_res.stats.total_cycles))
            * one_mill;
        let mcycle_price_max = (U256::from(order.request.offer.maxPrice)
            .saturating_sub(order_gas_cost)
            / U256::from(proof_res.stats.total_cycles))
            * one_mill;

        tracing::info!(
            "Order price: min: {} max: {} - cycles: {} - mcycle price: {} - {} - stake: {} gas_cost: {}",
            format_ether(U256::from(order.request.offer.minPrice)),
            format_ether(U256::from(order.request.offer.maxPrice)),
            proof_res.stats.total_cycles,
            format_ether(mcycle_price_min),
            format_ether(mcycle_price_max),
            order.request.offer.lockStake,
            format_ether(order_gas_cost),
        );

        // Skip the order if it will never be worth it
        if mcycle_price_max < config_min_mcycle_price {
            tracing::info!("Removing under priced order with request id {request_id:x}");
            return Ok(Skip);
        }

        let target_timestamp_secs = if mcycle_price_min >= config_min_mcycle_price {
            tracing::info!(
                "Selecting order with request id {request_id:x} at price {} - ASAP",
                format_ether(U256::from(order.request.offer.minPrice))
            );
            0 // Schedule the lock ASAP
        } else {
            let target_min_price =
                config_min_mcycle_price * (U256::from(proof_res.stats.total_cycles)) / one_mill
                    + order_gas_cost;
            tracing::debug!("Target price: {target_min_price} for request id {request_id:x}");

            order
                .request
                .offer
                .time_at_price(target_min_price)
                .context("Failed to get target price timestamp")?
        };

        let expiry_secs = order.request.offer.biddingStart + order.request.offer.lockTimeout as u64;

        Ok(Lock {
            total_cycles: Some(proof_res.stats.total_cycles),
            target_timestamp_secs,
            expiry_secs,
        })
    }

    /// Evaluate if a lock expired order is worth picking based on how much of the slashed stake token we can recover
    /// and the configured min mcycle price in stake tokens
    async fn evaluate_lock_expired_order(
        &self,
        order: &Order,
        proof_res: &ProofResult,
    ) -> Result<OrderPricingOutcome, PriceOrderErr> {
        let config_min_mcycle_price_stake_tokens = {
            let config = self.config.lock_all().context("Failed to read config")?;
            parse_ether(&config.market.mcycle_price_stake_token)
                .context("Failed to parse mcycle_price")?
        };

        let total_cycles = U256::from(proof_res.stats.total_cycles);

        // Reward for the order is a fraction of the stake once the lock has expired
        let one_mill = U256::from(1_000_000);
        let price = order.request.offer.stake_reward_if_locked_and_not_fulfilled();
        let mcycle_price_in_stake_tokens = price / total_cycles * one_mill;

        tracing::info!(
            "Order price: {} (stake tokens) - cycles: {} - mcycle price: {} (stake tokens), config_min_mcycle_price_stake_tokens: {} (stake tokens)",
            format_ether(price),
            proof_res.stats.total_cycles,
            format_ether(mcycle_price_in_stake_tokens),
            format_ether(config_min_mcycle_price_stake_tokens),
        );

        // Skip the order if it will never be worth it
        if mcycle_price_in_stake_tokens < config_min_mcycle_price_stake_tokens {
            tracing::info!(
                "Removing under priced order (slashed stake reward too low) {}",
                order.id()
            );
            return Ok(Skip);
        }

        Ok(ProveAfterLockExpire {
            total_cycles: Some(proof_res.stats.total_cycles),
            lock_expire_timestamp_secs: order.request.offer.biddingStart
                + order.request.offer.lockTimeout as u64,
            expiry_secs: order.request.offer.biddingStart + order.request.offer.timeout as u64,
        })
    }

    async fn find_existing_orders(&self) -> Result<()> {
        let pricing_orders = self
            .db
            .get_active_pricing_orders()
            .await
            .context("Failed to get active orders for pricing from db")?;

        tracing::info!("Found {} orders currently pricing to resume", pricing_orders.len());

        // TODO: This just restarts the process of preflight which is slightly wasteful
        // we should probably save off the preflight session ID into the DB and resume monitoring
        // like how we do in the prover.
        for order in pricing_orders {
            let self_copy = self.clone();
            tokio::spawn(async move { self_copy.price_order_and_update_db(&order).await });
        }

        Ok(())
    }

    /// Return the total amount of stake that is marked locally in the DB to be locked
    /// but has not yet been locked in the market contract thus has not been deducted from the account balance
    async fn pending_locked_stake(&self) -> Result<U256> {
        // NOTE: i64::max is the largest timestamp value possible in the DB.
        let pending_locks = self.db.get_pending_lock_orders(i64::MAX as u64).await?;
        let stake = pending_locks
            .iter()
            .map(|order| order.request.offer.lockStake)
            .fold(U256::ZERO, |acc, x| acc + x);
        Ok(stake)
    }

    /// Estimate of gas for locking a single order
    /// Currently just uses the config estimate but this may change in the future
    async fn estimate_gas_to_lock(&self, order: &Order) -> Result<u64> {
        let mut estimate =
            self.config.lock_all().context("Failed to read config")?.market.lockin_gas_estimate;

        if order.request.is_smart_contract_signed() {
            estimate += ERC1271_MAX_GAS_FOR_CHECK;
        }

        Ok(estimate)
    }

    /// Estimate of gas for to fulfill a single order
    /// Currently just uses the config estimate but this may change in the future
    async fn estimate_gas_to_fulfill(&self, order: &Order) -> Result<u64> {
        // TODO: Add gas costs for orders with large journals.
        let (base, groth16) = {
            let config = self.config.lock_all().context("Failed to read config")?;
            (config.market.fulfill_gas_estimate, config.market.groth16_verify_gas_estimate)
        };

        let mut estimate = base;

        // Add gas for orders that make use of the callbacks feature.
        estimate += u64::try_from(
            order
                .request
                .requirements
                .callback
                .as_option()
                .map(|callback| callback.gasLimit)
                .unwrap_or(U96::ZERO),
        )?;

        estimate += match self
            .supported_selectors
            .proof_type(order.request.requirements.selector)
            .context("unsupported selector")?
        {
            ProofType::Any | ProofType::Inclusion => 0,
            ProofType::Groth16 => groth16,
            proof_type => {
                tracing::warn!("Unknown proof type in gas cost estimation: {proof_type:?}");
                0
            }
        };

        Ok(estimate)
    }

    /// Estimate of gas for locking in any pending locks and submitting any pending proofs
    // NOTE: This could be optimized by storing the gas estimate on the DB order and using SQL to
    // sum it. Given that the number of concurrantly pending orders should be somewhat small, this
    // may not matter.
    async fn estimate_gas_to_lock_pending(&self) -> Result<u64> {
        let mut gas = 0;
        // NOTE: i64::max is the largest timestamp value possible in the DB.
        for order in self.db.get_pending_lock_orders(i64::MAX as u64).await?.iter() {
            tracing::debug!("Estimating gas to lock order with id {}", order.id());
            gas += self.estimate_gas_to_lock(order).await?;
        }
        Ok(gas)
    }

    /// Estimate of gas for fulfilling any orders either pending lock or locked
    // NOTE: This could be optimized by storing the gas estimate on the DB order and using SQL to
    // sum it. Given that the number of concurrantly pending orders should be somewhat small, this
    // may not matter.
    async fn estimate_gas_to_fulfill_pending(&self) -> Result<u64> {
        let mut gas = 0;
        for order in self.db.get_pending_fulfill_orders(i64::MAX as u64).await? {
            tracing::debug!("Estimating gas to fulfill order with id {}", order.id());
            gas += self.estimate_gas_to_fulfill(&order).await?;
        }
        Ok(gas)
    }

    /// Estimate the total gas tokens reserved to lock and fulfill all pending orders
    async fn gas_balance_reserved(&self) -> Result<U256> {
        let gas_price =
            self.chain_monitor.current_gas_price().await.context("Failed to get gas price")?;
        let lock_pending_gas = self.estimate_gas_to_lock_pending().await?;
        let fulfill_pending_gas = self.estimate_gas_to_fulfill_pending().await?;
        Ok(U256::from(gas_price) * U256::from(lock_pending_gas + fulfill_pending_gas))
    }

    /// Return available gas balance.
    ///
    /// This is defined as the balance of the signer account.
    async fn available_gas_balance(&self) -> Result<U256> {
        let balance = self
            .provider
            .get_balance(self.provider.default_signer_address())
            .await
            .context("Failed to get current wallet balance")?;

        let gas_balance_reserved = self.gas_balance_reserved().await?;

        let available = balance.saturating_sub(gas_balance_reserved);
        tracing::debug!(
            "available gas balance: (account_balance) {} - (expected_future_gas) {} = {}",
            format_ether(balance),
            format_ether(gas_balance_reserved),
            format_ether(available)
        );

        Ok(available)
    }

    /// Return available stake balance.
    ///
    /// This is defined as the balance in staking tokens of the signer account minus any pending locked stake.
    async fn available_stake_balance(&self) -> Result<U256> {
        let balance = self.market.balance_of_stake(self.provider.default_signer_address()).await?;
        let pending_balance = self.pending_locked_stake().await?;
        Ok(balance.saturating_sub(pending_balance))
    }

    async fn spawn_pricing_tasks(&self, tasks: &mut JoinSet<bool>, capacity: u32) -> Result<()> {
        let order_res = self.db.update_orders_for_pricing(capacity).await?;
        tracing::trace!(
            "Found {} orders to price, with order ids: {:?}",
            order_res.len(),
            order_res.iter().map(|order| order.id()).collect::<Vec<_>>()
        );

        for order in order_res {
            let picker_clone = self.clone();
            tasks.spawn(async move { picker_clone.price_order_and_update_db(&order).await });
        }
        Ok(())
    }
}

impl<P> RetryTask for OrderPicker<P>
where
    P: Provider<Ethereum> + 'static + Clone + WalletProvider,
{
    fn spawn(&self) -> RetryRes {
        let picker_copy = self.clone();

        Box::pin(async move {
            tracing::info!("Starting order picking monitor");

            picker_copy.find_existing_orders().await.map_err(SupervisorErr::Fault)?;

            // Use JoinSet to track active pricing tasks
            let mut pricing_tasks = JoinSet::new();

            // 5 second interval between spawning pricing tasks.
            let mut pricing_check_timer = tokio::time::interval_at(
                tokio::time::Instant::now(),
                tokio::time::Duration::from_secs(5),
            );

            loop {
                tokio::select! {
                    _ = pricing_check_timer.tick() => {
                        // Queue up orders that can be added to capacity.
                        let order_size = MAX_PRICING_BATCH_SIZE.saturating_sub(pricing_tasks.len() as u32);
                        tracing::trace!(
                            "Current active pricing tasks: {pricing_tasks:?}. Max possible: {MAX_PRICING_BATCH_SIZE}. Spawning up to {order_size} pricing tasks"
                        );
                        picker_copy
                            .spawn_pricing_tasks(&mut pricing_tasks, order_size)
                            .await
                            .map_err(SupervisorErr::Recover)?;
                    }

                    // Process completed pricing tasks
                    Some(result) = pricing_tasks.join_next() => {
                        tracing::trace!(
                            "Pricing task completed with result: {result:?}"
                        );
                        match result {
                            Ok(true) => {
                                // Order was priced successfully and will proceed to the next stage.
                            }
                            Ok(false) => {
                                // Order was not priced successfully and will be skipped.
                            }
                            Err(e) => {
                                return Err(SupervisorErr::Recover(anyhow::anyhow!("Pricing task failed: {e}")));
                            }
                        }
                    }
                }
            }
        })
    }
}

// DO NOT MERGE: Add a test that the order_gas_cost is being enforced as a min price.
#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        chain_monitor::ChainMonitorService, db::SqliteDb, provers::DefaultProver, FulfillmentType,
        OrderStatus,
    };
    use alloy::{
        network::EthereumWallet,
        node_bindings::{Anvil, AnvilInstance},
        primitives::{address, aliases::U96, Address, Bytes, FixedBytes, B256},
        providers::{ext::AnvilApi, ProviderBuilder},
        signers::local::PrivateKeySigner,
    };
    use boundless_market::contracts::{
        Callback, Input, Offer, Predicate, PredicateType, ProofRequest, RequestId, Requirements,
    };
    use boundless_market::storage::{MockStorageProvider, StorageProvider};
    use boundless_market_test_utils::{deploy_boundless_market, deploy_hit_points};
    use chrono::Utc;
    use guest_assessor::{ASSESSOR_GUEST_ID, ASSESSOR_GUEST_PATH};
    use guest_util::{ECHO_ELF, ECHO_ID};
    use risc0_ethereum_contracts::selector::Selector;
    use risc0_zkvm::sha::Digest;
    use tracing_test::traced_test;

    /// Reusable context for testing the order picker
    struct TestCtx<P> {
        anvil: AnvilInstance,
        picker: OrderPicker<P>,
        boundless_market: BoundlessMarketService<Arc<P>>,
        storage_provider: MockStorageProvider,
        db: DbObj,
        provider: Arc<P>,
    }

    /// Parameters for the generate_next_order function.
    struct OrderParams {
        pub order_index: u32,
        pub min_price: U256,
        pub max_price: U256,
        pub lock_stake: U256,
    }

    impl Default for OrderParams {
        fn default() -> Self {
            Self {
                order_index: 1,
                min_price: parse_ether("0.02").unwrap(),
                max_price: parse_ether("0.04").unwrap(),
                lock_stake: U256::ZERO,
            }
        }
    }

    impl<P> TestCtx<P>
    where
        P: Provider + WalletProvider,
    {
        fn signer(&self, index: usize) -> PrivateKeySigner {
            self.anvil.keys()[index].clone().into()
        }

        async fn generate_next_order(&self, params: OrderParams) -> Order {
            let image_url = self.storage_provider.upload_image(ECHO_ELF).await.unwrap();
            let image_id = Digest::from(ECHO_ID);
            let chain_id = self.provider.get_chain_id().await.unwrap();
            let boundless_market_address = self.boundless_market.instance().address();

            Order {
                status: OrderStatus::Pricing,
                updated_at: Utc::now(),
                request: ProofRequest::new(
                    RequestId::new(self.provider.default_signer_address(), params.order_index),
                    Requirements::new(
                        image_id,
                        Predicate {
                            predicateType: PredicateType::PrefixMatch,
                            data: Default::default(),
                        },
                    ),
                    image_url,
                    Input::builder().write_slice(&[0x41, 0x41, 0x41, 0x41]).build_inline().unwrap(),
                    Offer {
                        minPrice: params.min_price,
                        maxPrice: params.max_price,
                        biddingStart: now_timestamp(),
                        timeout: 1200,
                        lockTimeout: 900,
                        rampUpPeriod: 1,
                        lockStake: params.lock_stake,
                    },
                ),
                target_timestamp: None,
                image_id: None,
                input_id: None,
                proof_id: None,
                compressed_proof_id: None,
                expire_timestamp: None,
                client_sig: Bytes::new(),
                lock_price: None,
                fulfillment_type: FulfillmentType::LockAndFulfill,
                error_msg: None,
                boundless_market_address: *boundless_market_address,
                chain_id,
                total_cycles: None,
                proving_started_at: None,
            }
        }
    }

    #[derive(Default)]
    struct TestCtxBuilder {
        initial_signer_eth: Option<i32>,
        initial_hp: Option<U256>,
        config: Option<ConfigLock>,
    }

    impl TestCtxBuilder {
        fn with_initial_signer_eth(self, eth: i32) -> Self {
            Self { initial_signer_eth: Some(eth), ..self }
        }
        fn with_initial_hp(self, hp: U256) -> Self {
            assert!(hp < U256::from(U96::MAX), "Cannot have more than 2^96 hit points");
            Self { initial_hp: Some(hp), ..self }
        }
        fn with_config(self, config: ConfigLock) -> Self {
            Self { config: Some(config), ..self }
        }
        async fn build(self) -> TestCtx<impl Provider + WalletProvider + Clone + 'static> {
            let anvil = Anvil::new()
                .args(["--balance", &format!("{}", self.initial_signer_eth.unwrap_or(10000))])
                .spawn();
            let signer: PrivateKeySigner = anvil.keys()[0].clone().into();
            let provider = Arc::new(
                ProviderBuilder::new()
                    .wallet(EthereumWallet::from(signer.clone()))
                    .connect(&anvil.endpoint())
                    .await
                    .unwrap(),
            );

            provider.anvil_mine(Some(4), Some(2)).await.unwrap();

            let hp_contract = deploy_hit_points(signer.address(), provider.clone()).await.unwrap();
            let market_address = deploy_boundless_market(
                signer.address(),
                provider.clone(),
                Address::ZERO,
                hp_contract,
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

            if let Some(initial_hp) = self.initial_hp {
                tracing::debug!("Setting initial locked hitpoints to {}", initial_hp);
                boundless_market.deposit_stake_with_permit(initial_hp, &signer).await.unwrap();
                assert_eq!(
                    boundless_market
                        .balance_of_stake(provider.default_signer_address())
                        .await
                        .unwrap(),
                    initial_hp
                );
            }

            let storage_provider = MockStorageProvider::start();

            let db: DbObj = Arc::new(SqliteDb::new("sqlite::memory:").await.unwrap());
            let config = self.config.unwrap_or_default();
            let prover: ProverObj = Arc::new(DefaultProver::new());
            let chain_monitor = Arc::new(ChainMonitorService::new(provider.clone()).await.unwrap());
            tokio::spawn(chain_monitor.spawn());

            let picker = OrderPicker::new(
                db.clone(),
                config,
                prover,
                market_address,
                provider.clone(),
                chain_monitor,
            );

            TestCtx { anvil, picker, boundless_market, storage_provider, db, provider }
        }
    }

    #[tokio::test]
    #[traced_test]
    async fn price_order() {
        let config = ConfigLock::default();
        {
            config.load_write().unwrap().market.mcycle_price = "0.0000001".into();
        }
        let ctx = TestCtxBuilder::default().with_config(config).build().await;

        let order = ctx.generate_next_order(Default::default()).await;

        let _request_id =
            ctx.boundless_market.submit_request(&order.request, &ctx.signer(0)).await.unwrap();

        ctx.db.add_order(order.clone()).await.unwrap();
        let locked = ctx.picker.price_order_and_update_db(&order).await;
        assert!(locked);

        let db_order = ctx.db.get_order(&order.id()).await.unwrap().unwrap();
        assert_eq!(db_order.status, OrderStatus::WaitingToLock);
        assert_eq!(db_order.target_timestamp, Some(0));
    }

    #[tokio::test]
    #[traced_test]
    async fn skip_bad_predicate() {
        let config = ConfigLock::default();
        {
            config.load_write().unwrap().market.mcycle_price = "0.0000001".into();
        }
        let ctx = TestCtxBuilder::default().with_config(config).build().await;

        let mut order = ctx.generate_next_order(Default::default()).await;

        // set a bad predicate
        order.request.requirements.predicate =
            Predicate { predicateType: PredicateType::DigestMatch, data: B256::ZERO.into() };

        let _request_id =
            ctx.boundless_market.submit_request(&order.request, &ctx.signer(0)).await.unwrap();

        ctx.db.add_order(order.clone()).await.unwrap();
        let locked = ctx.picker.price_order_and_update_db(&order).await;
        assert!(!locked);

        let db_order = ctx.db.get_order(&order.id()).await.unwrap().unwrap();
        assert_eq!(db_order.status, OrderStatus::Skipped);

        assert!(logs_contain("predicate check failed, skipping"));
    }

    #[tokio::test]
    #[traced_test]
    async fn skip_unsupported_selector() {
        let config = ConfigLock::default();
        {
            config.load_write().unwrap().market.mcycle_price = "0.0000001".into();
        }
        let ctx = TestCtxBuilder::default().with_config(config).build().await;

        let mut order = ctx.generate_next_order(Default::default()).await;

        // set an unsupported selector
        order.request.requirements.selector = FixedBytes::from(Selector::Groth16V1_1 as u32);

        let _request_id =
            ctx.boundless_market.submit_request(&order.request, &ctx.signer(0)).await.unwrap();

        ctx.db.add_order(order.clone()).await.unwrap();
        let locked = ctx.picker.price_order_and_update_db(&order).await;
        assert!(!locked);

        let db_order = ctx.db.get_order(&order.id()).await.unwrap().unwrap();
        assert_eq!(db_order.status, OrderStatus::Skipped);

        assert!(logs_contain("has an unsupported selector requirement"));
    }

    #[tokio::test]
    #[traced_test]
    async fn skip_price_less_than_gas_costs() {
        let config = ConfigLock::default();
        {
            config.load_write().unwrap().market.mcycle_price = "0.0000001".into();
        }
        let ctx = TestCtxBuilder::default().with_config(config).build().await;

        let order = ctx
            .generate_next_order(OrderParams {
                min_price: parse_ether("0.0005").unwrap(),
                max_price: parse_ether("0.0010").unwrap(),
                ..Default::default()
            })
            .await;
        let request_id = order.request.id;

        let _request_id =
            ctx.boundless_market.submit_request(&order.request, &ctx.signer(0)).await.unwrap();

        ctx.db.add_order(order.clone()).await.unwrap();
        let locked = ctx.picker.price_order_and_update_db(&order).await;
        assert!(!locked);

        let db_order = ctx.db.get_order(&order.id()).await.unwrap().unwrap();
        assert_eq!(db_order.status, OrderStatus::Skipped);

        assert!(logs_contain(&format!(
            "Estimated gas cost to lock and fulfill request {request_id:x}:"
        )));
    }

    #[tokio::test]
    #[traced_test]
    async fn skip_price_less_than_gas_costs_groth16() {
        let config = ConfigLock::default();
        {
            config.load_write().unwrap().market.mcycle_price = "0.0000001".into();
        }
        let ctx = TestCtxBuilder::default().with_config(config).build().await;

        // NOTE: Values currently adjusted ad hoc to be between the two thresholds.
        let min_price = parse_ether("0.0013").unwrap();
        let max_price = parse_ether("0.0013").unwrap();

        // Order should have high enough price with the default selector.
        let order = ctx
            .generate_next_order(OrderParams {
                order_index: 1,
                min_price,
                max_price,
                ..Default::default()
            })
            .await;

        let _request_id =
            ctx.boundless_market.submit_request(&order.request, &ctx.signer(0)).await.unwrap();

        ctx.db.add_order(order.clone()).await.unwrap();
        let locked = ctx.picker.price_order_and_update_db(&order).await;
        assert!(locked);

        let db_order = ctx.db.get_order(&order.id()).await.unwrap().unwrap();
        assert_eq!(db_order.status, OrderStatus::WaitingToLock);
        assert_eq!(db_order.target_timestamp, Some(0));

        // Order does not have high enough price when groth16 is used.
        let mut order = ctx
            .generate_next_order(OrderParams {
                order_index: 2,
                min_price,
                max_price,
                ..Default::default()
            })
            .await;

        // set a Groth16 selector
        order.request.requirements.selector = FixedBytes::from(Selector::Groth16V2_0 as u32);
        let request_id = order.request.id;

        let _request_id =
            ctx.boundless_market.submit_request(&order.request, &ctx.signer(0)).await.unwrap();

        ctx.db.add_order(order.clone()).await.unwrap();
        let locked = ctx.picker.price_order_and_update_db(&order).await;
        assert!(!locked);

        let db_order = ctx.db.get_order(&order.id()).await.unwrap().unwrap();
        assert_eq!(db_order.status, OrderStatus::Skipped);

        assert!(logs_contain(&format!(
            "Estimated gas cost to lock and fulfill request {request_id:x}:"
        )));
    }

    #[tokio::test]
    #[traced_test]
    async fn skip_price_less_than_gas_costs_callback() {
        let config = ConfigLock::default();
        {
            config.load_write().unwrap().market.mcycle_price = "0.0000001".into();
        }
        let ctx = TestCtxBuilder::default().with_config(config).build().await;

        // NOTE: Values currently adjusted ad hoc to be between the two thresholds.
        let min_price = parse_ether("0.0013").unwrap();
        let max_price = parse_ether("0.0013").unwrap();

        // Order should have high enough price with the default selector.
        let order = ctx
            .generate_next_order(OrderParams {
                order_index: 1,
                min_price,
                max_price,
                ..Default::default()
            })
            .await;
        let _request_id =
            ctx.boundless_market.submit_request(&order.request, &ctx.signer(0)).await.unwrap();

        ctx.db.add_order(order.clone()).await.unwrap();
        let locked = ctx.picker.price_order_and_update_db(&order).await;
        assert!(locked);

        let db_order = ctx.db.get_order(&order.id()).await.unwrap().unwrap();
        assert_eq!(db_order.status, OrderStatus::WaitingToLock);
        assert_eq!(db_order.target_timestamp, Some(0));

        // Order does not have high enough price when groth16 is used.
        let mut order = ctx
            .generate_next_order(OrderParams {
                order_index: 2,
                min_price,
                max_price,
                ..Default::default()
            })
            .await;

        // set a callback with a nontrivial gas consumption
        order.request.requirements.callback = Callback {
            addr: address!("0x00000000000000000000000000000000ca11bac2"),
            gasLimit: U96::from(200_000),
        };
        let request_id = order.request.id;

        let _request_id =
            ctx.boundless_market.submit_request(&order.request, &ctx.signer(0)).await.unwrap();

        ctx.db.add_order(order.clone()).await.unwrap();
        let locked = ctx.picker.price_order_and_update_db(&order).await;
        assert!(!locked);

        let db_order = ctx.db.get_order(&order.id()).await.unwrap().unwrap();
        assert_eq!(db_order.status, OrderStatus::Skipped);

        assert!(logs_contain(&format!(
            "Estimated gas cost to lock and fulfill request {request_id:x}:"
        )));
    }

    #[tokio::test]
    #[traced_test]
    async fn skip_price_less_than_gas_costs_smart_contract_signature() {
        let config = ConfigLock::default();
        {
            config.load_write().unwrap().market.mcycle_price = "0.0000001".into();
        }
        let ctx = TestCtxBuilder::default().with_config(config).build().await;

        // NOTE: Values currently adjusted ad hoc to be between the two thresholds.
        let min_price = parse_ether("0.0013").unwrap();
        let max_price = parse_ether("0.0013").unwrap();

        // Order should have high enough price with the default selector.
        let order = ctx
            .generate_next_order(OrderParams {
                order_index: 1,
                min_price,
                max_price,
                ..Default::default()
            })
            .await;

        let _request_id =
            ctx.boundless_market.submit_request(&order.request, &ctx.signer(0)).await.unwrap();

        ctx.db.add_order(order.clone()).await.unwrap();
        let locked = ctx.picker.price_order_and_update_db(&order).await;
        assert!(locked);

        let db_order = ctx.db.get_order(&order.id()).await.unwrap().unwrap();
        assert_eq!(db_order.status, OrderStatus::WaitingToLock);
        assert_eq!(db_order.target_timestamp, Some(0));

        // Order does not have high enough price when groth16 is used.
        let mut order = ctx
            .generate_next_order(OrderParams {
                order_index: 2,
                min_price,
                max_price,
                ..Default::default()
            })
            .await;

        order.request.id =
            RequestId::try_from(order.request.id).unwrap().set_smart_contract_signed_flag().into();
        let request_id = order.request.id;

        let _request_id =
            ctx.boundless_market.submit_request(&order.request, &ctx.signer(0)).await.unwrap();

        ctx.db.add_order(order.clone()).await.unwrap();
        let locked = ctx.picker.price_order_and_update_db(&order).await;
        assert!(!locked);

        let db_order = ctx.db.get_order(&order.id()).await.unwrap().unwrap();
        assert_eq!(db_order.status, OrderStatus::Skipped);

        assert!(logs_contain(&format!(
            "Estimated gas cost to lock and fulfill request {request_id:x}:"
        )));
    }

    #[tokio::test]
    #[traced_test]
    async fn skip_unallowed_addr() {
        let config = ConfigLock::default();
        {
            config.load_write().unwrap().market.mcycle_price = "0.0000001".into();
            config.load_write().unwrap().market.allow_client_addresses = Some(vec![Address::ZERO]);
        }
        let ctx = TestCtxBuilder::default().with_config(config).build().await;

        let order = ctx.generate_next_order(Default::default()).await;

        let _request_id =
            ctx.boundless_market.submit_request(&order.request, &ctx.signer(0)).await.unwrap();

        ctx.db.add_order(order.clone()).await.unwrap();
        let locked = ctx.picker.price_order_and_update_db(&order).await;
        assert!(!locked);

        let db_order = ctx.db.get_order(&order.id()).await.unwrap().unwrap();
        assert_eq!(db_order.status, OrderStatus::Skipped);

        assert!(logs_contain("because it is not in allowed addrs"));
    }

    #[tokio::test]
    #[traced_test]
    async fn resume_order_pricing() {
        let config = ConfigLock::default();
        {
            config.load_write().unwrap().market.mcycle_price = "0.0000001".into();
        }
        let ctx = TestCtxBuilder::default().with_config(config).build().await;

        let order = ctx.generate_next_order(Default::default()).await;

        let _request_id =
            ctx.boundless_market.submit_request(&order.request, &ctx.signer(0)).await.unwrap();

        ctx.db.add_order(order.clone()).await.unwrap();

        ctx.picker.find_existing_orders().await.unwrap();

        assert!(logs_contain("Found 1 orders currently pricing to resume"));

        // Try and wait for the order to complete pricing
        for _ in 0..4 {
            let db_order = ctx.db.get_order(&order.id()).await.unwrap().unwrap();
            if db_order.status != OrderStatus::Pricing {
                break;
            }
            tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
        }

        let db_order = ctx.db.get_order(&order.id()).await.unwrap().unwrap();
        assert_eq!(db_order.status, OrderStatus::WaitingToLock);
        assert_eq!(db_order.target_timestamp, Some(0));
    }

    // TODO: Test
    // need to test the non-ASAP path for pricing, aka picking a timestamp ahead in time to make sure
    // that price calculator is working correctly.

    #[tokio::test]
    #[traced_test]
    async fn pending_locked_stake() {
        let lock_stake = U256::from(10);

        let config = ConfigLock::default();
        {
            config.load_write().unwrap().market.mcycle_price = "0.0000001".into();
            config.load_write().unwrap().market.max_stake = "10".into();
        }

        let ctx = TestCtxBuilder::default()
            .with_config(config)
            .with_initial_hp(U256::from(100))
            .build()
            .await;
        assert_eq!(ctx.picker.pending_locked_stake().await.unwrap(), U256::ZERO);

        let order = ctx.generate_next_order(OrderParams { lock_stake, ..Default::default() }).await;

        ctx.db.add_order(order.clone()).await.unwrap();
        let locked = ctx.picker.price_order_and_update_db(&order).await;
        assert!(locked);
        // order is pending lock so stake is counted
        assert_eq!(ctx.picker.pending_locked_stake().await.unwrap(), lock_stake);

        ctx.db.set_proving_status_lock_and_fulfill_orders(&order.id(), U256::ZERO).await.unwrap();
        // order no longer pending lock so stake no longer counted
        assert_eq!(ctx.picker.pending_locked_stake().await.unwrap(), U256::ZERO);
    }

    #[tokio::test]
    #[traced_test]
    async fn use_gas_to_lock_estimate_from_config() {
        let lockin_gas = 123_456;
        let config = ConfigLock::default();
        {
            config.load_write().unwrap().market.mcycle_price = "0.0000001".into();
            config.load_write().unwrap().market.lockin_gas_estimate = lockin_gas;
        }

        let ctx = TestCtxBuilder::default().with_config(config).build().await;
        assert_eq!(ctx.picker.pending_locked_stake().await.unwrap(), U256::ZERO);

        let order = ctx.generate_next_order(Default::default()).await;
        assert_eq!(ctx.picker.estimate_gas_to_lock(&order).await.unwrap(), lockin_gas);

        ctx.db.add_order(order.clone()).await.unwrap();
        let locked = ctx.picker.price_order_and_update_db(&order).await;
        assert!(locked);

        assert_eq!(ctx.picker.estimate_gas_to_lock_pending().await.unwrap(), lockin_gas);
    }

    #[tokio::test]
    #[traced_test]
    async fn use_gas_to_fulfill_estimate_from_config() {
        let fulfill_gas = 123_456;
        let config = ConfigLock::default();
        {
            config.load_write().unwrap().market.mcycle_price = "0.0000001".into();
            config.load_write().unwrap().market.fulfill_gas_estimate = fulfill_gas;
        }

        let ctx = TestCtxBuilder::default().with_config(config).build().await;
        assert_eq!(ctx.picker.pending_locked_stake().await.unwrap(), U256::ZERO);

        let order = ctx.generate_next_order(Default::default()).await;
        ctx.db.add_order(order.clone()).await.unwrap();
        let locked = ctx.picker.price_order_and_update_db(&order).await;
        assert!(locked);

        assert_eq!(ctx.picker.estimate_gas_to_fulfill_pending().await.unwrap(), fulfill_gas);

        // add another order
        let order =
            ctx.generate_next_order(OrderParams { order_index: 2, ..Default::default() }).await;
        ctx.db.add_order(order.clone()).await.unwrap();
        let locked = ctx.picker.price_order_and_update_db(&order).await;
        assert!(locked);

        // gas estimate stacks (until estimates factor in bundling)
        assert_eq!(ctx.picker.estimate_gas_to_fulfill_pending().await.unwrap(), 2 * fulfill_gas);
    }

    #[tokio::test]
    #[traced_test]
    async fn pending_order_gas_estimation() {
        let lockin_gas = 1000;
        let fulfill_gas = 50000;
        let config = ConfigLock::default();
        {
            config.load_write().unwrap().market.mcycle_price = "0.0000001".into();
            config.load_write().unwrap().market.fulfill_gas_estimate = fulfill_gas;
            config.load_write().unwrap().market.lockin_gas_estimate = lockin_gas;
        }

        let ctx = TestCtxBuilder::default().with_config(config).build().await;
        assert_eq!(ctx.picker.pending_locked_stake().await.unwrap(), U256::ZERO);

        let order = ctx.generate_next_order(Default::default()).await;
        ctx.db.add_order(order.clone()).await.unwrap();
        let priced = ctx.picker.price_order_and_update_db(&order).await;
        assert!(priced);

        let gas_price = ctx.provider.get_gas_price().await.unwrap();
        assert_eq!(
            ctx.picker.gas_balance_reserved().await.unwrap(),
            U256::from(gas_price) * U256::from(fulfill_gas + lockin_gas)
        );
        // mark the order as locked.
        ctx.db.set_proving_status_lock_and_fulfill_orders(&order.id(), U256::ZERO).await.unwrap();
        // only fulfillment gas now reserved
        assert_eq!(
            ctx.picker.gas_balance_reserved().await.unwrap(),
            U256::from(gas_price) * U256::from(fulfill_gas)
        );
    }

    #[tokio::test]
    #[traced_test]
    async fn cannot_overcommit_stake() {
        let signer_inital_balance_eth = 2;
        let lockin_stake = U256::from(150);

        let config = ConfigLock::default();
        {
            config.load_write().unwrap().market.mcycle_price = "0.0000001".into();
            config.load_write().unwrap().market.max_stake = "10".into();
        }

        let ctx = TestCtxBuilder::default()
            .with_initial_signer_eth(signer_inital_balance_eth)
            .with_initial_hp(lockin_stake)
            .with_config(config)
            .build()
            .await;
        let order = ctx
            .generate_next_order(OrderParams { lock_stake: U256::from(100), ..Default::default() })
            .await;
        let mut orders: Vec<Order> = vec![order.clone(), order.clone()];

        for (i, order) in orders.iter_mut().enumerate() {
            order.request.id = U256::from(i);
            ctx.db.add_order(order.clone()).await.unwrap();
            ctx.picker.price_order_and_update_db(order).await;
        }

        // only the first order above should have marked as active pricing, the second one should have been skipped due to insufficient stake
        assert_eq!(
            ctx.db.get_order(&orders[0].id()).await.unwrap().unwrap().status,
            OrderStatus::WaitingToLock
        );
        assert_eq!(
            ctx.db.get_order(&orders[1].id()).await.unwrap().unwrap().status,
            OrderStatus::Skipped
        );
        assert!(logs_contain("Insufficient available stake to lock order"));
    }

    #[tokio::test]
    #[traced_test]
    async fn skips_journal_exceeding_limit() {
        // set this by testing a very small limit (1 byte)
        let config = ConfigLock::default();
        {
            config.load_write().unwrap().market.mcycle_price = "0.0000001".into();
            config.load_write().unwrap().market.max_journal_bytes = 1;
        }
        let lock_stake = U256::from(10);

        let ctx =
            TestCtxBuilder::default().with_config(config).with_initial_hp(lock_stake).build().await;
        let order = ctx.generate_next_order(OrderParams { lock_stake, ..Default::default() }).await;

        ctx.db.add_order(order.clone()).await.unwrap();
        let locked = ctx.picker.price_order_and_update_db(&order).await;
        assert!(!locked);

        assert_eq!(
            ctx.db.get_order(&order.id()).await.unwrap().unwrap().status,
            OrderStatus::Skipped
        );
        assert!(logs_contain("journal larger than set limit"));
    }

    #[tokio::test]
    #[traced_test]
    async fn price_locked_by_other() {
        let config = ConfigLock::default();
        {
            config.load_write().unwrap().market.mcycle_price_stake_token = "0.0000001".into();
        }
        let ctx = TestCtxBuilder::default()
            .with_config(config)
            .with_initial_hp(U256::from(1000))
            .build()
            .await;

        let mut order = ctx.generate_next_order(Default::default()).await;

        order.status = OrderStatus::Pricing;
        order.request.offer.biddingStart = now_timestamp();
        order.request.offer.lockTimeout = 1000;
        order.request.offer.timeout = 10000;
        order.request.offer.lockStake = parse_ether("0.1").unwrap();
        order.fulfillment_type = FulfillmentType::FulfillAfterLockExpire;
        let order_id = order.request.id;
        ctx.db.add_order(order.clone()).await.unwrap();

        assert!(ctx.picker.price_order_and_update_db(&order).await);

        assert!(logs_contain(&format!(
            "Setting order with request id {:x} to prove after lock expiry at {}",
            order_id,
            order.request.offer.biddingStart + order.request.offer.lockTimeout as u64
        )));

        let expected_target_timestamp =
            order.request.offer.biddingStart + order.request.offer.lockTimeout as u64;
        let expected_expire_timestamp =
            order.request.offer.biddingStart + order.request.offer.timeout as u64;
        let db_order = ctx.db.get_order(&order.id()).await.unwrap().unwrap();
        assert_eq!(db_order.status, OrderStatus::WaitingForLockToExpire);
        assert_eq!(db_order.target_timestamp, Some(expected_target_timestamp));
        assert_eq!(db_order.expire_timestamp, Some(expected_expire_timestamp));
    }

    #[tokio::test]
    #[traced_test]
    async fn price_locked_by_other_unprofitable() {
        let config = ConfigLock::default();
        {
            config.load_write().unwrap().market.mcycle_price_stake_token = "0.1".into();
        }
        let ctx = TestCtxBuilder::default().with_config(config).build().await;

        let mut order = ctx.generate_next_order(Default::default()).await;

        order.status = OrderStatus::Pricing;
        order.fulfillment_type = FulfillmentType::FulfillAfterLockExpire;
        order.request.offer.biddingStart = now_timestamp();
        order.request.offer.lockTimeout = 0;
        order.request.offer.timeout = 10000;

        // Low stake means low reward for filling after it is unfulfilled
        order.request.offer.lockStake = parse_ether("0.00001").unwrap();

        let request_id = order.request.id;
        ctx.db.add_order(order.clone()).await.unwrap();

        assert!(!ctx.picker.price_order_and_update_db(&order).await);

        // Since we know the stake reward is constant, and we know our min_mycle_price_stake_token
        // the execution limit check tells us if the order is profitable or not, since it computes the max number
        // of cycles that can be proven while keeping the order profitable.
        assert!(logs_contain(&format!(
            "Skipping order {:x} due to session limit exceeded",
            request_id
        )));

        let db_order = ctx.db.get_order(&order.id()).await.unwrap().unwrap();
        assert_eq!(db_order.status, OrderStatus::Skipped);
    }

    // Currently the logic this test intends to check (the lines in evaluate_lock_expired_order) are unreachable,
    // since the only way to hit it would be to skip the preflight check, and the current skip preflight configuration
    // doesn't check the price at all.
    // TODO: Confirm expected behavior for skip_preflight
    #[traced_test]
    #[ignore]
    #[allow(dead_code)]
    async fn price_locked_by_other_unprofitable_with_skip_preflight_ids() {
        let config = ConfigLock::default();
        {
            config.load_write().unwrap().market.mcycle_price_stake_token = "0.1".into();
            config.load_write().unwrap().market.skip_preflight_ids =
                vec![Digest::from(ECHO_ID).as_bytes().try_into().unwrap()].into();
        }
        let ctx = TestCtxBuilder::default().with_config(config).build().await;

        let mut order = ctx.generate_next_order(Default::default()).await;

        order.status = OrderStatus::Pricing;
        order.fulfillment_type = FulfillmentType::FulfillAfterLockExpire;
        order.request.offer.biddingStart = now_timestamp();
        order.request.offer.lockTimeout = 0;
        order.request.offer.timeout = 10000;

        // Low stake means low reward for filling after it is unfulfilled
        order.request.offer.lockStake = parse_ether("0.00001").unwrap();

        ctx.db.add_order(order.clone()).await.unwrap();

        assert!(!ctx.picker.price_order_and_update_db(&order).await);

        assert!(logs_contain("Removing under priced order (slashed stake reward too low)"));

        let db_order = ctx.db.get_order(&order.id()).await.unwrap().unwrap();
        assert_eq!(db_order.status, OrderStatus::Skipped);
    }
}
