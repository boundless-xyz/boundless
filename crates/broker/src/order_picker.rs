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
    Order,
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

use OrderPricingOutcome::{Lock, ProveImmediate, Skip};

// fraction the stake the protocol gives to the prover who fills an order that was locked by another prover but expired
// e.g. a value of 1 means 1/4 of the original stake is given to the prover who fills the order.
// This is determined by the constant SLASHING_BURN_BPS defined in the BoundlessMarket contract.
// The value is 4 because the slashing burn is 75% of the stake, and we give the remaining 1/4 of that to the prover.
// TODO: Retrieve this from the contract in the future
const FRACTION_STAKE_REWARD: u64 = 4;

/// Maximum number of orders to concurrently work on pricing. Used to limit pricing tasks spawned.
const MAX_PRICING_BATCH_SIZE: u32 = 10;

/// Gas allocated to verifying a smart contract signature. Copied from BoundlessMarket.sol.
const ERC1271_MAX_GAS_FOR_CHECK: u64 = 100000;

#[derive(Error, Debug)]
#[non_exhaustive]
pub enum PriceOrderErr {
    #[error("failed to fetch / push input")]
    FetchInputErr(#[source] anyhow::Error),

    #[error("failed to fetch / push image")]
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
    // Tracks the timestamp when the prover estimates it will complete the locked orders.
    prover_available_at: Arc<tokio::sync::Mutex<u64>>,
}

#[derive(Debug)]
#[non_exhaustive]
enum OrderPricingOutcome {
    // Order should be locked and proving commence after lock is secured
    Lock {
        target_timestamp_secs: u64,
        // TODO handle checking what time the lock should occur before, when estimating proving time.
        expiry_secs: u64,
    },
    // Do not lock the order but attempt to prove and fulfill it
    ProveImmediate,
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
            prover_available_at: Arc::new(tokio::sync::Mutex::new(now_timestamp())),
        }
    }

    async fn price_order_and_update_db(&self, order_id: U256, order: &Order) -> bool {
        let f = || async {
            match self.price_order(order_id, order).await {
                Ok(Lock { target_timestamp_secs, expiry_secs }) => {
                    tracing::debug!("Locking order {order_id:x}");
                    self.db
                        .set_order_lock(order_id, target_timestamp_secs, expiry_secs)
                        .await
                        .context("Failed to set_order_lock")?;
                    Ok::<_, PriceOrderErr>(true)
                }
                Ok(ProveImmediate) => {
                    tracing::debug!("Proving order {order_id:x} to submit immediately");
                    self.db
                        .set_proving_status(order_id, U256::ZERO)
                        .await
                        .context("Failed to set_proving_status")?;
                    Ok(true)
                }
                Ok(Skip) => {
                    tracing::debug!("Skipping order {order_id:x}");
                    self.db.skip_order(order_id).await.context("Failed to delete order")?;
                    Ok(false)
                }
                Err(err) => {
                    tracing::error!("Failed to price order {order_id:x}: {err:?}");
                    self.db
                        .set_order_failure(order_id, err.to_string())
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
                tracing::error!("Failed to update db for order {order_id:x}: {err:?}");
                false
            }
        }
    }

    async fn price_order(
        &self,
        order_id: U256,
        order: &Order,
    ) -> Result<OrderPricingOutcome, PriceOrderErr> {
        tracing::debug!("Processing order {order_id:x}: {order:?}");

        let (min_deadline, allowed_addresses_opt) = {
            let config = self.config.lock_all().context("Failed to read config")?;
            (config.market.min_deadline, config.market.allow_client_addresses.clone())
        };

        // Initial sanity checks:
        if let Some(allow_addresses) = allowed_addresses_opt {
            let client_addr = order.request.client_address();
            if !allow_addresses.contains(&client_addr) {
                tracing::info!("Removing order {order_id:x} from {client_addr} because it is not in allowed addrs");
                return Ok(Skip);
            }
        }

        if !self.supported_selectors.is_supported(order.request.requirements.selector) {
            tracing::info!(
                "Removing order {order_id:x} because it has an unsupported selector requirement"
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
        let lock_expired = lock_expiration <= now && order_expiration > now;

        if lock_expired {
            tracing::info!("Order {order_id:x} lock period has expired but it is unfulfilled");
        }

        let (expiration, lockin_stake) = if lock_expired {
            (order_expiration, U256::ZERO)
        } else {
            (lock_expiration, U256::from(order.request.offer.lockStake))
        };

        if expiration <= now {
            tracing::info!("Removing order {order_id:x} because it has expired");
            return Ok(Skip);
        };

        // Does the order expire within the min deadline
        let seconds_left = expiration - now;
        if seconds_left <= min_deadline {
            tracing::info!("Removing order {order_id:x} because it expires within the deadline left: {seconds_left} deadline: {min_deadline}");
            return Ok(Skip);
        }

        // Check if the stake is sane and if we can afford it
        let max_stake = {
            let config = self.config.lock_all().context("Failed to read config")?;
            parse_ether(&config.market.max_stake).context("Failed to parse max_stake")?
        };

        if lockin_stake > max_stake {
            tracing::info!("Removing high stake order {order_id:x}");
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
            "Estimated {order_gas} gas to lock and fill order {order_id:x}; {} ether @ {} gwei",
            format_ether(order_gas_cost),
            format_units(gas_price, "gwei").unwrap()
        );

        if order_gas_cost > order.request.offer.maxPrice && !lock_expired {
            // Cannot check the gas cost for lock expired orders where the reward is a fraction of the stake
            // TODO: This can be added once we have a price feed for the stake token in gas tokens
            tracing::info!(
                "Estimated gas cost to lock and fill order {order_id:x}: {} exceeds max price; max price {}",
                format_ether(order_gas_cost),
                format_ether(order.request.offer.maxPrice)
            );
            self.db.skip_order(order_id).await.context("Failed to delete order")?;
            return Ok(Skip);
        }

        if order_gas_cost > available_gas {
            tracing::warn!("Estimated there will be insufficient gas for order {order_id:x} after locking and fulfilling pending orders; available_gas {} ether", format_ether(available_gas));
            self.db.skip_order(order_id).await.context("Failed to delete order")?;
            return Ok(Skip);
        }

        if lockin_stake > available_stake {
            tracing::warn!(
                "Insufficient available stake to lock order {order_id:x}. Requires {lockin_stake}, has {available_stake}"
            );
            return Ok(Skip);
        }

        let (skip_preflight, peak_prove_khz, max_mcycle_limit) = {
            let config = self.config.lock_all().context("Failed to read config")?;
            let skip_preflight =
                if let Some(skip_preflights) = config.market.skip_preflight_ids.as_ref() {
                    skip_preflights.contains(&order.request.requirements.imageId)
                } else {
                    false
                };

            (skip_preflight, config.market.peak_prove_khz, config.market.max_mcycle_limit)
        };

        if skip_preflight {
            // If we skip preflight we lockin the order asap
            if lock_expired {
                return Ok(ProveImmediate);
            } else {
                return Ok(Lock { target_timestamp_secs: 0, expiry_secs: expiration });
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
            .set_image_input_ids(order_id, &image_id, &input_id)
            .await
            .context("Failed to record Input/Image IDs to DB")?;

        // Create a executor limit based on the max price of the order
        let exec_limit: u64 = if lock_expired {
            let mcycle_price_stake_token = {
                let config = self.config.lock_all().context("Failed to read config")?;
                parse_ether(&config.market.mcycle_price_stake_token)
                    .context("Failed to parse mcycle_price")?
            };
            // Note this does not account for gas cost unlike a normal order
            // TODO: Update to account for gas once the stake token to gas token exchange rate is known
            let price = order.request.offer.lockStake / U256::from(FRACTION_STAKE_REWARD);
            if mcycle_price_stake_token == U256::ZERO {
                u64::MAX / 1024 / 1024 // max limit is ok we don't care about proving costs
            } else {
                (price / mcycle_price_stake_token)
                    .try_into()
                    .context("Failed to convert U256 exec limit to u64")?
            }
        } else {
            let config_min_mcycle_price = {
                let config = self.config.lock_all().context("Failed to read config")?;
                parse_ether(&config.market.mcycle_price).context("Failed to parse mcycle_price")?
            };

            (U256::from(order.request.offer.maxPrice).saturating_sub(order_gas_cost)
                / config_min_mcycle_price)
                .try_into()
                .context("Failed to convert U256 exec limit to u64")?
        };

        if exec_limit == 0 {
            tracing::info!(
                "Removing order {order_id:x} because it's mcycle price limit is below 0 mcycles"
            );

            return Ok(Skip);
        }

        tracing::debug!(
            "Starting preflight execution of {order_id:x} exec limit {exec_limit} mcycles"
        );
        // TODO add a future timeout here to put a upper bound on how long to preflight for
        let proof_res = self
            .prover
            .preflight(
                &image_id,
                &input_id,
                vec![],
                /* TODO assumptions */ Some(exec_limit * 1024 * 1024),
            )
            .await
            .map_err(|err| match err {
                ProverError::ProvingFailed(ref err_msg) => {
                    // TODO: Get enum'd errors from the SDK to prevent str
                    // checks
                    if err_msg.contains("GuestPanic") {
                        PriceOrderErr::GuestPanic(err_msg.clone())
                    } else {
                        PriceOrderErr::OtherErr(err.into())
                    }
                }
                _ => PriceOrderErr::OtherErr(err.into()),
            })?;

        // If a max_mcycle_limit is configured check if the order is over that limit
        if let Some(mcycle_limit) = max_mcycle_limit {
            let mcycles = proof_res.stats.total_cycles / 1_000_000;
            if mcycles >= mcycle_limit {
                tracing::info!("Order {order_id:x} max_mcycle_limit check failed req: {mcycle_limit} | config: {mcycles}");
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
                "Order {order_id:x} journal larger than set limit ({} > {}), skipping",
                journal.len(),
                max_journal_bytes
            );
            return Ok(Skip);
        }

        // Validate the predicates:
        if !order.request.requirements.predicate.eval(journal.clone()) {
            tracing::info!("Order {order_id:x} predicate check failed, skipping");
            return Ok(Skip);
        }

        // Check if the order can be completed before its deadline
        if let Some(peak_prove_khz) = peak_prove_khz {
            // TODO: this is a naive solution for the following reasons:
            // 1. Time estimate based on `peak_prove_khz`, which may not be the actual proving time
            // 2. This doesn't take into account the aggregation proving time
            // 3. Doesn't account for non-proving slop
            // 4. Assumes proofs are prioritized by order of scheduling, and may be cases where a
            //    previously locked order cannot complete within the deadline if more orders locked.
            // So if using, a conservative peak_prove_khz should be used.

            // Calculate how long this proof will take to complete in seconds, rounded up.
            let proof_time_seconds =
                (proof_res.stats.total_cycles.div_ceil(1_000)).div_ceil(peak_prove_khz);

            // Get the current prover availability time
            let mut prover_available = self.prover_available_at.lock().await;
            let start_time = std::cmp::max(*prover_available, now);
            let completion_time = start_time + proof_time_seconds;

            if completion_time >= expiration {
                drop(prover_available);
                // Proof estimated that it cannot complete before the expiration
                tracing::info!(
                    "Order {order_id:x} cannot be completed in time. Proof estimated to take {proof_time_seconds}s to complete, would be {}s past deadline",
                    completion_time.saturating_sub(expiration)
                );
                return Ok(Skip);
            }

            *prover_available = completion_time;
            drop(prover_available);
            tracing::debug!("Order {order_id:x} estimated to take {proof_time_seconds}s to prove");

            let res = self
                .evaluate_order(order_id, order, &proof_res, order_gas_cost, lock_expired)
                .await;
            if let Err(e) = res {
                // Failed to select order, decrement the reserved capacity.
                let mut prover_available = self.prover_available_at.lock().await;
                *prover_available = prover_available.saturating_sub(proof_time_seconds);
                return Err(e);
            }
            res
        } else {
            self.evaluate_order(order_id, order, &proof_res, order_gas_cost, lock_expired).await
        }
    }

    async fn evaluate_order(
        &self,
        order_id: U256,
        order: &Order,
        proof_res: &ProofResult,
        order_gas_cost: U256,
        lock_expired: bool,
    ) -> Result<OrderPricingOutcome, PriceOrderErr> {
        if lock_expired {
            return self.evaluate_lock_expired_order(order_id, order, proof_res).await;
        } else {
            self.evaluate_lockable_order(order_id, order, proof_res, order_gas_cost).await
        }
    }

    /// Evaluate if a regular lockable order is worth picking based on the price and the configured min mcycle price
    async fn evaluate_lockable_order(
        &self,
        order_id: U256,
        order: &Order,
        proof_res: &ProofResult,
        order_gas_cost: U256,
    ) -> Result<OrderPricingOutcome, PriceOrderErr> {
        let config_min_mcycle_price = {
            let config = self.config.lock_all().context("Failed to read config")?;
            parse_ether(&config.market.mcycle_price).context("Failed to parse mcycle_price")?
        };

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
            tracing::info!("Removing under priced order {order_id:x}");
            return Ok(Skip);
        }

        let target_timestamp_secs = if mcycle_price_min >= config_min_mcycle_price {
            tracing::info!(
                "Selecting order {order_id:x} at price {} - ASAP",
                format_ether(U256::from(order.request.offer.minPrice))
            );
            0 // Schedule the lock ASAP
        } else {
            let target_min_price =
                config_min_mcycle_price * (U256::from(proof_res.stats.total_cycles)) / one_mill
                    + order_gas_cost;
            tracing::debug!("Target price: {target_min_price}");

            order
                .request
                .offer
                .time_at_price(target_min_price)
                .context("Failed to get target price timestamp")?
        };

        let expiry_secs = order.request.offer.biddingStart + order.request.offer.lockTimeout as u64;

        Ok(Lock { target_timestamp_secs, expiry_secs })
    }

    /// Evaluate if a lock expired order is worth picking based on how much of the slashed stake token we can recover
    /// and the configured min mcycle price in stake tokens
    async fn evaluate_lock_expired_order(
        &self,
        order_id: U256,
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
        let price = order.request.offer.lockStake / U256::from(FRACTION_STAKE_REWARD);
        let mcycle_price_in_stake_tokens = price / total_cycles * one_mill;

        tracing::info!(
            "Order price: {} (stake tokens) - cycles: {} - mcycle price: {} (stake tokens)",
            format_ether(price),
            proof_res.stats.total_cycles,
            format_ether(mcycle_price_in_stake_tokens),
        );

        // Skip the order if it will never be worth it
        if mcycle_price_in_stake_tokens < config_min_mcycle_price_stake_tokens {
            tracing::info!("Removing under priced order {order_id:x}");
            return Ok(Skip);
        }

        Ok(ProveImmediate)
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
        for (order_id, order) in pricing_orders {
            let self_copy = self.clone();
            tokio::spawn(
                async move { self_copy.price_order_and_update_db(order_id, &order).await },
            );
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
            .map(|(_, order)| order.request.offer.lockStake)
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
        for (_, order) in self.db.get_pending_lock_orders(i64::MAX as u64).await?.iter() {
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
        for (_, order) in self.db.get_committed_orders().await? {
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
        Ok(balance - pending_balance)
    }

    async fn get_pricing_order_capacity(&self) -> Result<Capacity> {
        let max_concurrent_locks = {
            let config = self.config.lock_all()?;
            config.market.max_concurrent_locks
        };

        if let Some(max) = max_concurrent_locks {
            let committed_orders_count = self.db.get_committed_orders_count().await?;
            let available_slots = max.saturating_sub(committed_orders_count);
            if committed_orders_count == 0 {
                Ok(Capacity::Idle(available_slots))
            } else {
                Ok(Capacity::PartiallyLocked(available_slots))
            }
        } else {
            Ok(Capacity::Unlimited)
        }
    }

    async fn spawn_pricing_tasks(&self, tasks: &mut JoinSet<bool>, capacity: u32) -> Result<()> {
        if capacity == 0 {
            return Ok(());
        }

        let order_res = self.db.update_orders_for_pricing(capacity, now_timestamp()).await?;

        for (order_id, order) in order_res {
            let picker_clone = self.clone();
            tasks.spawn(
                async move { picker_clone.price_order_and_update_db(order_id, &order).await },
            );
        }

        Ok(())
    }
}

/// The capacity of the order picker, if there is a limit on the number of concurrent locks.
#[derive(Debug, PartialEq)]
enum Capacity {
    /// There are no pending or currently locked orders.
    Idle(u32),
    /// There are orders that are picked to be locked but not fulfilled yet.
    PartiallyLocked(u32),
    /// There is no concurrent lock limit.
    Unlimited,
}

impl Capacity {
    /// Returns the number of orders to request from the DB to price. Capped at
    /// [MAX_PRICING_BATCH_SIZE] to limit pricing tasks spawned.
    fn request_size(&self, pricing_tasks: usize) -> u32 {
        match self {
            Capacity::Idle(capacity) | Capacity::PartiallyLocked(capacity) => std::cmp::min(
                capacity.saturating_sub(u32::try_from(pricing_tasks).expect("tasks u32 overflow")),
                MAX_PRICING_BATCH_SIZE,
            ),
            Capacity::Unlimited => MAX_PRICING_BATCH_SIZE,
        }
    }
    fn increment_locked_order(&mut self) {
        match self {
            Capacity::Idle(capacity) | Capacity::PartiallyLocked(capacity) => {
                *self = Capacity::PartiallyLocked(capacity.saturating_sub(1))
            }
            Capacity::Unlimited => (),
        }
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

            // Set capacity at 0, to ensure the capacity is read before scheduling orders.
            // Assume orders are partially locked until read from DB.
            let mut capacity = Capacity::PartiallyLocked(0u32);

            // Check for config updates and current lock count periodically
            let config_check_interval = tokio::time::Duration::from_secs(10);
            let mut config_check_timer = tokio::time::interval(config_check_interval);

            // 5 second interval with a 2.5 second delay so config is checked first,
            // and that the requests are interleaved between config checks.
            let pricing_check_interval = tokio::time::Duration::from_secs(5);
            let mut pricing_check_timer = tokio::time::interval_at(
                tokio::time::Instant::now() + tokio::time::Duration::from_millis(2500),
                pricing_check_interval,
            );

            loop {
                if matches!(capacity, Capacity::Idle(_)) && pricing_tasks.is_empty() {
                    // All orders complete and no pricing tasks in flight, set the
                    // prover available estimate to current time to ensure that the
                    // broker does not wait idle.
                    tracing::trace!(
                        "No in progress orders, setting prover available to current time"
                    );
                    *picker_copy.prover_available_at.lock().await = now_timestamp();
                }
                tokio::select! {
                    _ = config_check_timer.tick() => {
                        // Get updated max concurrent locks and calculate capacity based on orders
                        // that are locked but not fulfilled yet.
                        capacity = picker_copy
                            .get_pricing_order_capacity()
                            .await
                            .map_err(SupervisorErr::Recover)?;

                        tracing::trace!("Updated capacity to {capacity:?}");
                    }

                    _ = pricing_check_timer.tick() => {
                        // Queue up orders that can be added to capacity.
                        let order_size = capacity.request_size(pricing_tasks.len());

                        tracing::trace!("Spawing {} pricing tasks - {} tasks already running", order_size, pricing_tasks.len());
                        picker_copy
                            .spawn_pricing_tasks(&mut pricing_tasks, order_size)
                            .await
                            .map_err(SupervisorErr::Recover)?;
                    }

                    // Process completed pricing tasks
                    Some(result) = pricing_tasks.join_next() => {
                        tracing::trace!("Pricing task completed with result: {result:?}");
                        match result {
                            Ok(true) => {
                                capacity.increment_locked_order();
                            }
                            Ok(false) => {
                                // Order was not selected for locking.
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
        chain_monitor::ChainMonitorService, db::SqliteDb, provers::DefaultProver, OrderStatus,
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
                error_msg: None,
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

        let order_id = order.request.id;
        ctx.db.add_order(order_id, order.clone()).await.unwrap();
        let locked = ctx.picker.price_order_and_update_db(order_id, &order).await;
        assert!(locked);

        let db_order = ctx.db.get_order(order_id).await.unwrap().unwrap();
        assert_eq!(db_order.status, OrderStatus::Locking);
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
        let order_id = order.request.id;

        // set a bad predicate
        order.request.requirements.predicate =
            Predicate { predicateType: PredicateType::DigestMatch, data: B256::ZERO.into() };

        let _request_id =
            ctx.boundless_market.submit_request(&order.request, &ctx.signer(0)).await.unwrap();

        ctx.db.add_order(order_id, order.clone()).await.unwrap();
        let locked = ctx.picker.price_order_and_update_db(order_id, &order).await;
        assert!(!locked);

        let db_order = ctx.db.get_order(order_id).await.unwrap().unwrap();
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
        let order_id = order.request.id;

        // set an unsupported selector
        order.request.requirements.selector = FixedBytes::from(Selector::Groth16V1_1 as u32);

        let _request_id =
            ctx.boundless_market.submit_request(&order.request, &ctx.signer(0)).await.unwrap();

        ctx.db.add_order(order_id, order.clone()).await.unwrap();
        let locked = ctx.picker.price_order_and_update_db(order_id, &order).await;
        assert!(!locked);

        let db_order = ctx.db.get_order(order_id).await.unwrap().unwrap();
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
        let order_id = order.request.id;

        let _request_id =
            ctx.boundless_market.submit_request(&order.request, &ctx.signer(0)).await.unwrap();

        ctx.db.add_order(order_id, order.clone()).await.unwrap();
        let locked = ctx.picker.price_order_and_update_db(order_id, &order).await;
        assert!(!locked);

        let db_order = ctx.db.get_order(order_id).await.unwrap().unwrap();
        assert_eq!(db_order.status, OrderStatus::Skipped);

        assert!(logs_contain(&format!("Estimated gas cost to lock and fill order {order_id:x}:")));
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
        let order_id = order.request.id;

        let _request_id =
            ctx.boundless_market.submit_request(&order.request, &ctx.signer(0)).await.unwrap();

        ctx.db.add_order(order_id, order.clone()).await.unwrap();
        let locked = ctx.picker.price_order_and_update_db(order_id, &order).await;
        assert!(locked);

        let db_order = ctx.db.get_order(order_id).await.unwrap().unwrap();
        assert_eq!(db_order.status, OrderStatus::Locking);
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
        let order_id = order.request.id;

        let _request_id =
            ctx.boundless_market.submit_request(&order.request, &ctx.signer(0)).await.unwrap();

        ctx.db.add_order(order_id, order.clone()).await.unwrap();
        let locked = ctx.picker.price_order_and_update_db(order_id, &order).await;
        assert!(!locked);

        let db_order = ctx.db.get_order(order_id).await.unwrap().unwrap();
        assert_eq!(db_order.status, OrderStatus::Skipped);

        assert!(logs_contain(&format!("Estimated gas cost to lock and fill order {order_id:x}:")));
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
        let order_id = order.request.id;

        let _request_id =
            ctx.boundless_market.submit_request(&order.request, &ctx.signer(0)).await.unwrap();

        ctx.db.add_order(order_id, order.clone()).await.unwrap();
        let locked = ctx.picker.price_order_and_update_db(order_id, &order).await;
        assert!(locked);

        let db_order = ctx.db.get_order(order_id).await.unwrap().unwrap();
        assert_eq!(db_order.status, OrderStatus::Locking);
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
        let order_id = order.request.id;

        let _request_id =
            ctx.boundless_market.submit_request(&order.request, &ctx.signer(0)).await.unwrap();

        ctx.db.add_order(order_id, order.clone()).await.unwrap();
        let locked = ctx.picker.price_order_and_update_db(order_id, &order).await;
        assert!(!locked);

        let db_order = ctx.db.get_order(order_id).await.unwrap().unwrap();
        assert_eq!(db_order.status, OrderStatus::Skipped);

        assert!(logs_contain(&format!("Estimated gas cost to lock and fill order {order_id:x}:")));
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
        let order_id = order.request.id;

        let _request_id =
            ctx.boundless_market.submit_request(&order.request, &ctx.signer(0)).await.unwrap();

        ctx.db.add_order(order_id, order.clone()).await.unwrap();
        let locked = ctx.picker.price_order_and_update_db(order_id, &order).await;
        assert!(locked);

        let db_order = ctx.db.get_order(order_id).await.unwrap().unwrap();
        assert_eq!(db_order.status, OrderStatus::Locking);
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
        let order_id = order.request.id;

        let _request_id =
            ctx.boundless_market.submit_request(&order.request, &ctx.signer(0)).await.unwrap();

        ctx.db.add_order(order_id, order.clone()).await.unwrap();
        let locked = ctx.picker.price_order_and_update_db(order_id, &order).await;
        assert!(!locked);

        let db_order = ctx.db.get_order(order_id).await.unwrap().unwrap();
        assert_eq!(db_order.status, OrderStatus::Skipped);

        assert!(logs_contain(&format!("Estimated gas cost to lock and fill order {order_id:x}:")));
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
        let order_id = order.request.id;

        let _request_id =
            ctx.boundless_market.submit_request(&order.request, &ctx.signer(0)).await.unwrap();

        ctx.db.add_order(order_id, order.clone()).await.unwrap();
        let locked = ctx.picker.price_order_and_update_db(order_id, &order).await;
        assert!(!locked);

        let db_order = ctx.db.get_order(order_id).await.unwrap().unwrap();
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
        let order_id = order.request.id;

        let _request_id =
            ctx.boundless_market.submit_request(&order.request, &ctx.signer(0)).await.unwrap();

        ctx.db.add_order(order_id, order.clone()).await.unwrap();

        ctx.picker.find_existing_orders().await.unwrap();

        assert!(logs_contain("Found 1 orders currently pricing to resume"));

        // Try and wait for the order to complete pricing
        for _ in 0..4 {
            let db_order = ctx.db.get_order(order_id).await.unwrap().unwrap();
            if db_order.status != OrderStatus::Pricing {
                break;
            }
            tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
        }

        let db_order = ctx.db.get_order(order_id).await.unwrap().unwrap();
        assert_eq!(db_order.status, OrderStatus::Locking);
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
        let order_id = order.request.id;

        ctx.db.add_order(order_id, order.clone()).await.unwrap();
        let locked = ctx.picker.price_order_and_update_db(order_id, &order).await;
        assert!(locked);
        // order is pending lock so stake is counted
        assert_eq!(ctx.picker.pending_locked_stake().await.unwrap(), lock_stake);

        ctx.db.set_proving_status(order_id, U256::ZERO).await.unwrap();
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
        let order_id = order.request.id;
        assert_eq!(ctx.picker.estimate_gas_to_lock(&order).await.unwrap(), lockin_gas);

        ctx.db.add_order(order_id, order.clone()).await.unwrap();
        let locked = ctx.picker.price_order_and_update_db(order_id, &order).await;
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
        let order_id = order.request.id;
        ctx.db.add_order(order_id, order.clone()).await.unwrap();
        let locked = ctx.picker.price_order_and_update_db(order_id, &order).await;
        assert!(locked);

        assert_eq!(ctx.picker.estimate_gas_to_fulfill_pending().await.unwrap(), fulfill_gas);

        // add another order
        let order =
            ctx.generate_next_order(OrderParams { order_index: 2, ..Default::default() }).await;
        let order_id = order.request.id;
        ctx.db.add_order(order_id, order.clone()).await.unwrap();
        let locked = ctx.picker.price_order_and_update_db(order_id, &order).await;
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
        let order_id = order.request.id;
        ctx.db.add_order(order_id, order.clone()).await.unwrap();
        let locked = ctx.picker.price_order_and_update_db(order_id, &order).await;
        assert!(locked);

        let gas_price = ctx.provider.get_gas_price().await.unwrap();
        assert_eq!(
            ctx.picker.gas_balance_reserved().await.unwrap(),
            U256::from(gas_price) * U256::from(fulfill_gas + lockin_gas)
        );
        // mark the order as locked.
        ctx.db.set_proving_status(order_id, U256::ZERO).await.unwrap();
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
        let orders = std::iter::repeat(order).take(2);

        for (order_id, order) in orders.into_iter().enumerate() {
            let order_id = U256::from(order_id);
            ctx.db.add_order(order_id, order.clone()).await.unwrap();
            ctx.picker.price_order_and_update_db(order_id, &order).await;
        }

        // only the first order above should have marked as active pricing, the second one should have been skipped due to insufficient stake
        assert_eq!(
            ctx.db.get_order(U256::from(0)).await.unwrap().unwrap().status,
            OrderStatus::Locking
        );
        assert_eq!(
            ctx.db.get_order(U256::from(1)).await.unwrap().unwrap().status,
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

        let order_id = order.request.id;
        ctx.db.add_order(order_id, order.clone()).await.unwrap();
        let locked = ctx.picker.price_order_and_update_db(order_id, &order).await;
        assert!(!locked);

        assert_eq!(ctx.db.get_order(order_id).await.unwrap().unwrap().status, OrderStatus::Skipped);
        assert!(logs_contain("journal larger than set limit"));
    }

    #[tokio::test]
    #[traced_test]
    async fn accept_order_that_completes_before_expiration() {
        let config = ConfigLock::default();
        {
            let mut config_write = config.load_write().unwrap();
            config_write.market.mcycle_price = "0.0000001".into();
            config_write.market.peak_prove_khz = Some(1);
            config_write.market.min_deadline = 0;
        }
        let ctx = TestCtxBuilder::default().with_config(config).build().await;

        let mut order = ctx.generate_next_order(Default::default()).await;
        let order_id = order.request.id;

        // Modify the order to have a longer expiration time
        let current_time = now_timestamp();
        order.request.offer.biddingStart = current_time;
        order.request.offer.lockTimeout = 60;

        let _request_id =
            ctx.boundless_market.submit_request(&order.request, &ctx.signer(0)).await.unwrap();

        ctx.db.add_order(order_id, order.clone()).await.unwrap();

        let locked = ctx.picker.price_order_and_update_db(order_id, &order).await;
        assert!(locked);

        let db_order = ctx.db.get_order(order_id).await.unwrap().unwrap();
        assert_eq!(db_order.status, OrderStatus::Locking);

        // Verify that the debug log contains the estimated proving time
        assert!(logs_contain("estimated to take 4s to prove"));
    }

    #[tokio::test]
    #[traced_test]
    async fn orders_queue_up_completion_times() {
        let config = ConfigLock::default();
        {
            let mut config_write = config.load_write().unwrap();
            config_write.market.mcycle_price = "0.0000001".into();
            config_write.market.peak_prove_khz = Some(1);
            config_write.market.min_deadline = 0;
        }
        let ctx = TestCtxBuilder::default().with_config(config).build().await;

        // First order
        let mut order1 = ctx.generate_next_order(Default::default()).await;
        let order_id1 = order1.request.id;
        let current_time = now_timestamp();
        order1.request.offer.biddingStart = current_time;
        order1.request.offer.lockTimeout = 6;

        ctx.db.add_order(order_id1, order1.clone()).await.unwrap();
        let locked = ctx.picker.price_order_and_update_db(order_id1, &order1).await;
        assert!(locked);

        // Second order will be rejected because it would finish after its deadline with first order
        let mut order2 =
            ctx.generate_next_order(OrderParams { order_index: 2, ..Default::default() }).await;
        let order_id2 = order2.request.id;

        order2.request.offer.biddingStart = current_time;
        order2.request.offer.lockTimeout = 6;

        ctx.db.add_order(order_id2, order2.clone()).await.unwrap();
        let locked = ctx.picker.price_order_and_update_db(order_id2, &order2).await;
        assert!(!locked);

        // Check results
        assert_eq!(
            ctx.db.get_order(order_id1).await.unwrap().unwrap().status,
            OrderStatus::Locking
        );
        assert_eq!(
            ctx.db.get_order(order_id2).await.unwrap().unwrap().status,
            OrderStatus::Skipped
        );

        assert!(logs_contain("cannot be completed in time"));
        assert!(logs_contain("Proof estimated to take 4s to complete"));
        assert!(logs_contain("s past deadline"));
    }

    #[tokio::test]
    #[traced_test]
    async fn respects_max_concurrent_locks() {
        let max_concurrent_locks = 2;
        let config = ConfigLock::default();
        {
            let mut config_write = config.load_write().unwrap();
            config_write.market.mcycle_price = "0.0000001".into();
            config_write.market.max_concurrent_locks = Some(max_concurrent_locks);
        }

        let ctx = TestCtxBuilder::default()
            .with_config(config)
            .with_initial_hp(U256::from(1000))
            .build()
            .await;

        let mut orders = vec![
            ctx.generate_next_order(OrderParams {
                order_index: 1,
                lock_stake: U256::from(10),
                ..Default::default()
            })
            .await,
            ctx.generate_next_order(OrderParams {
                order_index: 2,
                lock_stake: U256::from(10),
                ..Default::default()
            })
            .await,
            ctx.generate_next_order(OrderParams {
                order_index: 3,
                lock_stake: U256::from(10),
                ..Default::default()
            })
            .await,
            ctx.generate_next_order(OrderParams {
                order_index: 4,
                lock_stake: U256::from(10),
                ..Default::default()
            })
            .await,
        ];

        for order in &mut orders {
            let order_id = order.request.id;

            // By default, testing infrastructure sets generated orders to `Pricing`
            order.status = OrderStatus::New;
            ctx.db.add_order(order_id, order.clone()).await.unwrap();
        }

        let capacity = ctx.picker.get_pricing_order_capacity().await.unwrap();
        assert_eq!(capacity, Capacity::Idle(max_concurrent_locks));

        let mut pricing_tasks = JoinSet::new();

        let request_size = capacity.request_size(pricing_tasks.len());
        assert_eq!(request_size, 2);
        ctx.picker.spawn_pricing_tasks(&mut pricing_tasks, request_size).await.unwrap();

        // Verify only up to max_concurrent_locks are being priced
        assert_eq!(pricing_tasks.len(), 2);

        // Finish pricing an order and mark it as complete to free up capacity
        let locked = pricing_tasks.join_next().await.unwrap().unwrap();
        assert!(locked);
        // Complete pricing other order, to ensure no race conditions in the test where db updated
        // while task still exists in joinset
        let locked = pricing_tasks.join_next().await.unwrap().unwrap();
        assert!(locked);

        // Set one of the in progress orders to complete to free up capacity
        ctx.db.set_order_complete(orders[0].request.id).await.unwrap();

        let capacity = ctx.picker.get_pricing_order_capacity().await.unwrap();
        assert_eq!(capacity, Capacity::PartiallyLocked(1));
        assert_eq!(pricing_tasks.len(), 0);

        let request_size = capacity.request_size(pricing_tasks.len());
        assert_eq!(request_size, 1);
        ctx.picker.spawn_pricing_tasks(&mut pricing_tasks, request_size).await.unwrap();
        assert_eq!(pricing_tasks.len(), 1);

        let order = ctx.db.get_order(orders[3].request.id).await.unwrap().unwrap();
        assert!(order.status == OrderStatus::New);
    }

    #[tokio::test]
    #[traced_test]
    async fn price_slashed_unfulfilled_order() {
        let config = ConfigLock::default();
        {
            config.load_write().unwrap().market.mcycle_price_stake_token = "0.0000001".into();
        }
        let ctx = TestCtxBuilder::default().with_config(config).build().await;

        let min_price = U256::from(200000000000u64);
        let max_price = U256::from(400000000000u64);

        let mut order = ctx
            .generate_next_order(OrderParams {
                order_index: 1,
                min_price,
                max_price,
                ..Default::default()
            })
            .await;

        order.status = OrderStatus::New;
        order.request.offer.biddingStart = now_timestamp();
        order.request.offer.lockTimeout = 0;
        order.request.offer.timeout = 10000;
        order.request.offer.lockStake = parse_ether("0.1").unwrap();

        let order_id = order.request.id;
        ctx.db.add_order(order_id, order.clone()).await.unwrap();

        assert!(ctx.picker.price_order_and_update_db(order_id, &order).await);

        assert!(logs_contain(&format!(
            "Order {:x} lock period has expired but it is unfulfilled",
            order_id
        )));

        let db_order = ctx.db.get_order(order_id).await.unwrap().unwrap();
        assert_eq!(db_order.status, OrderStatus::PendingProving);
    }

    #[tokio::test]
    #[traced_test]
    async fn price_unprofitable_slashed_unfulfilled_order_if_configured() {
        let config = ConfigLock::default();
        {
            config.load_write().unwrap().market.mcycle_price_stake_token = "0".into();
        }
        let ctx = TestCtxBuilder::default().with_config(config).build().await;

        let min_price = U256::from(200000000000u64);
        let max_price = U256::from(400000000000u64);

        let mut order = ctx
            .generate_next_order(OrderParams {
                order_index: 1,
                min_price,
                max_price,
                ..Default::default()
            })
            .await;

        order.status = OrderStatus::New;
        order.request.offer.biddingStart = now_timestamp();
        order.request.offer.lockTimeout = 0;
        order.request.offer.timeout = 10000;
        order.request.offer.lockStake = parse_ether("0.1").unwrap(); // no stake means no reward for filling after it is slashed

        let order_id = order.request.id;
        ctx.db.add_order(order_id, order.clone()).await.unwrap();
        ctx.picker.price_order(order_id, &order).await.unwrap();

        assert!(logs_contain(&format!(
            "Order {:x} lock period has expired but it is unfulfilled",
            order_id
        )));

        assert!(ctx.picker.price_order_and_update_db(order_id, &order).await);

        let db_order = ctx.db.get_order(order_id).await.unwrap().unwrap();
        assert_eq!(db_order.status, OrderStatus::PendingProving);
    }
}
