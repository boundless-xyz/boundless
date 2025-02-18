// Copyright (c) 2025 RISC Zero, Inc.
//
// All rights reserved.

use std::sync::Arc;

use crate::chain_monitor::ChainMonitorService;
use alloy::{
    network::Ethereum,
    primitives::{
        utils::{format_ether, parse_ether},
        Address, U256,
    },
    providers::{Provider, WalletProvider},
    transports::BoxTransport,
};
use anyhow::{Context, Result};
use boundless_market::contracts::{boundless_market::BoundlessMarketService, RequestError};
use thiserror::Error;

#[derive(Error, Debug)]
#[non_exhaustive]
pub enum PriceOrderErr {
    #[error("Failed to fetch / push input: {0}")]
    FetchInputErr(anyhow::Error),

    #[error("Failed to fetch / push image: {0}")]
    FetchImageErr(anyhow::Error),

    #[error("Guest execution faulted: {0}")]
    GuestPanic(String),

    #[error("Request: {0}")]
    RequestError(#[from] RequestError),

    #[error("Other: {0}")]
    OtherErr(#[from] anyhow::Error),
}

use crate::{
    config::ConfigLock,
    db::DbObj,
    provers::{ProverError, ProverObj},
    task::{RetryRes, RetryTask, SupervisorErr},
    Order,
};

#[derive(Clone)]
pub struct OrderPicker<P> {
    db: DbObj,
    config: ConfigLock,
    prover: ProverObj,
    provider: Arc<P>,
    chain_monitor: Arc<ChainMonitorService<P>>,
    block_time: u64,
    market: BoundlessMarketService<BoxTransport, Arc<P>>,
}

impl<P> OrderPicker<P>
where
    P: Provider<BoxTransport, Ethereum> + 'static + Clone + WalletProvider,
{
    pub fn new(
        db: DbObj,
        config: ConfigLock,
        prover: ProverObj,
        block_time: u64,
        market_addr: Address,
        provider: Arc<P>,
        chain_monitor: Arc<ChainMonitorService<P>>,
    ) -> Self {
        let market = BoundlessMarketService::new(
            market_addr,
            provider.clone(),
            provider.default_signer_address(),
        );
        Self { db, config, prover, chain_monitor, block_time, provider, market }
    }

    async fn price_order(&self, order_id: U256, order: &Order) -> Result<(), PriceOrderErr> {
        tracing::debug!("Processing order {order_id:x}: {order:?}");

        let (min_deadline, allowed_addresses_opt) = {
            let config = self.config.lock_all().context("Failed to read config")?;
            (config.market.min_deadline, config.market.allow_client_addresses.clone())
        };

        let current_block = self.chain_monitor.current_block_number().await?;

        // Initial sanity checks:
        if let Some(allow_addresses) = allowed_addresses_opt {
            let client_addr = order.request.client_address()?;
            if !allow_addresses.contains(&client_addr) {
                tracing::warn!("Removing order {order_id:x} from {client_addr} because it is not in allowed addrs");
                self.db.skip_order(order_id).await.context("Order not in allowed addr list")?;
                return Ok(());
            }
        }

        // is the order expired already?

        let expire_block = order.request.offer.biddingStart + order.request.offer.timeout as u64;

        if expire_block <= current_block {
            tracing::warn!("Removing order {order_id:x} because it has expired");
            self.db.skip_order(order_id).await.context("Failed to delete expired order")?;
            return Ok(());
        };

        // Does the order expire within the min deadline
        let seconds_left = (expire_block - current_block) * self.block_time;
        if seconds_left <= min_deadline {
            tracing::warn!("Removing order {order_id:x} because it expires within the deadline left: {seconds_left} deadline: {min_deadline}");
            self.db.skip_order(order_id).await.context("Failed to delete short deadline order")?;
            return Ok(());
        }

        // Check if the stake is sane and if we can afford it
        let max_stake = {
            let config = self.config.lock_all().context("Failed to read config")?;
            parse_ether(&config.market.max_stake).context("Failed to parse max_stake")?
        };

        let lockin_stake = U256::from(order.request.offer.lockStake);
        if lockin_stake > max_stake {
            tracing::warn!("Removing high stake order {order_id:x}");
            self.db.skip_order(order_id).await.context("Failed to delete order")?;
            return Ok(());
        }

        // Check that we have both enough staking tokens to stake, and enough gas tokens to lock and fulfil
        let gas_price = self.provider.get_gas_price().await.context("Failed to get gas price")?;
        let gas_to_lock_order =
            U256::from(gas_price) * U256::from(self.estimate_gas_to_lock(order).await?);
        let available_gas = self.available_gas_balance().await?;
        let available_stake = self.available_stake_balance().await?;

        if gas_to_lock_order > available_gas {
            tracing::warn!("Estimated there will be insufficient gas to lock this order after locking and fulfilling pending orders");
            self.db.skip_order(order_id).await.context("Failed to delete order")?;
            return Ok(());
        }
        if lockin_stake > available_stake {
            tracing::warn!(
                "Insufficient available stake to lock order {order_id:x}. Requires {lockin_stake}, has {available_stake}"
            );
            self.db.skip_order(order_id).await.context("Failed to delete order")?;
            return Ok(());
        }

        let (skip_preflight, max_size, peak_prove_khz, fetch_retries, max_mcycle_limit) = {
            let config = self.config.lock_all().context("Failed to read config")?;
            let skip_preflight =
                if let Some(skip_preflights) = config.market.skip_preflight_ids.as_ref() {
                    skip_preflights.contains(&order.request.requirements.imageId)
                } else {
                    false
                };

            (
                skip_preflight,
                config.market.max_file_size,
                config.market.peak_prove_khz,
                config.market.max_fetch_retries,
                config.market.max_mcycle_limit,
            )
        };

        if skip_preflight {
            // If we skip preflight we lockin the order asap
            self.db
                .set_order_lock(order_id, order.request.offer.biddingStart, expire_block)
                .await
                .with_context(|| format!("Failed to set_order_lock for order {order_id:x}"))?;
            return Ok(());
        }

        // TODO: Move URI handling like this into the prover impls
        let image_id = crate::upload_image_uri(&self.prover, order, max_size, fetch_retries)
            .await
            .map_err(PriceOrderErr::FetchImageErr)?;

        let input_id = crate::upload_input_uri(&self.prover, order, max_size, fetch_retries)
            .await
            .map_err(PriceOrderErr::FetchInputErr)?;

        // Record the image/input IDs for proving stage
        self.db
            .set_image_input_ids(order_id, &image_id, &input_id)
            .await
            .context("Failed to record Input/Image IDs to DB")?;

        // Create a executor limit based on the max price of the order
        let config_min_mcycle_price = {
            let config = self.config.lock_all().context("Failed to read config")?;
            parse_ether(&config.market.mcycle_price).context("Failed to parse mcycle_price")?
        };

        let exec_limit: u64 = (U256::from(order.request.offer.maxPrice) / config_min_mcycle_price)
            .try_into()
            .context("Failed to convert U256 exec limit to u64")?;

        if exec_limit == 0 {
            tracing::warn!(
                "Removing order {order_id:x} because it's mcycle price limit is below 0 mcycles"
            );
            self.db
                .skip_order(order_id)
                .await
                .context("Order max price below min mcycle price, limit 0")?;
            return Ok(());
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
                tracing::warn!("Order {order_id:x} max_mcycle_limit check failed req: {mcycle_limit} | config: {mcycles}");
                self.db.skip_order(order_id).await.context("Failed to delete order")?;
                return Ok(());
            }
        }

        // TODO: this only checks that we could prove this at peak_khz, not if the cluster currently
        // can absorb that proving load, we need to cordinate this check with parallel
        // proofs and the current state of Bento
        if let Some(prove_khz) = peak_prove_khz {
            let required_khz = (proof_res.stats.total_cycles / 1_000) / seconds_left;
            tracing::debug!("peak_prove_khz checking: {prove_khz} required: {required_khz}");
            if required_khz >= prove_khz {
                tracing::warn!("Order {order_id:x} peak_prove_khz check failed req: {required_khz} | config: {prove_khz}");
                self.db.skip_order(order_id).await.context("Failed to delete order")?;
                return Ok(());
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
            tracing::warn!(
                "Order {order_id:x} journal larger than set limit ({} > {}), skipping",
                journal.len(),
                max_journal_bytes
            );
            self.db.skip_order(order_id).await.context("Failed to delete order")?;
            return Ok(());
        }

        // Validate the predicates:
        if !order.request.requirements.predicate.eval(journal.clone()) {
            tracing::warn!("Order {order_id:x} predicate check failed, skipping");
            self.db.skip_order(order_id).await.context("Failed to delete order")?;
            return Ok(());
        }

        let one_mill = U256::from(1_000_000);

        let mcycle_price_min = (U256::from(order.request.offer.minPrice)
            / U256::from(proof_res.stats.total_cycles))
            * one_mill;
        let mcycle_price_max = (U256::from(order.request.offer.maxPrice)
            / U256::from(proof_res.stats.total_cycles))
            * one_mill;

        tracing::info!(
            "Order price: min: {} max: {} - cycles: {} - mcycle price: {} - {} - stake: {}",
            format_ether(U256::from(order.request.offer.minPrice)),
            format_ether(U256::from(order.request.offer.maxPrice)),
            proof_res.stats.total_cycles,
            format_ether(mcycle_price_min),
            format_ether(mcycle_price_max),
            order.request.offer.lockStake,
        );

        // Skip the order if it will never be worth it
        if mcycle_price_max < config_min_mcycle_price {
            tracing::warn!("Removing under priced order {order_id:x}");
            self.db.skip_order(order_id).await.context("Failed to delete order")?;
            return Ok(());
        }

        if mcycle_price_min >= config_min_mcycle_price {
            tracing::info!(
                "Selecting order {order_id:x} at price {} - ASAP",
                format_ether(U256::from(order.request.offer.minPrice))
            );
            // set the target block to a past block (aka the order block or current)
            // so we schedule the lock ASAP.
            self.db
                .set_order_lock(order_id, order.request.offer.biddingStart, expire_block)
                .await
                .with_context(|| format!("Failed to set_order_lock for order {order_id:x}"))?;
        }
        // Here we have to pick a target block that the price would be at our target price
        // TODO: Clean up and do more testing on this since its just a rough shot first draft
        else {
            let target_min_price =
                config_min_mcycle_price * (U256::from(proof_res.stats.total_cycles)) / one_mill;
            tracing::debug!("Target price: {target_min_price}");

            let target_block: u64 = self
                .market
                .block_at_price(&order.request.offer, target_min_price)
                .context("Failed to get target price block")?;
            tracing::info!(
                "Selecting order {order_id:x} at price {} - at block {}",
                format_ether(target_min_price),
                target_block,
            );

            self.db
                .set_order_lock(order_id, target_block, expire_block)
                .await
                .with_context(|| format!("Failed to set_order_lock for order {order_id:x}"))?;
        }

        Ok(())
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
            tokio::spawn(async move {
                if let Err(err) = self_copy.price_order(order_id, &order).await {
                    self_copy
                        .db
                        .set_order_failure(order_id, format!("{err:?}"))
                        .await
                        .expect("Failed to set DB failure");
                }
            });
        }

        Ok(())
    }

    /// Return the total amount of stake that is marked locally in the DB to be locked
    /// but has not yet been locked in the market contract thus has not been deducted from the account balance
    async fn pending_locked_stake(&self) -> Result<U256> {
        let pending_locks = self.db.get_pending_lock_orders(0).await?;
        let stake = pending_locks
            .iter()
            .map(|(_, order)| order.request.offer.lockStake)
            .fold(U256::ZERO, |acc, x| acc + x);
        Ok(stake)
    }

    /// Estimate of gas for locking a single order
    /// Currently just uses the config estimate but this may change in the future
    async fn estimate_gas_to_lock(&self, _order: &Order) -> Result<u64> {
        Ok(self.config.lock_all().context("Failed to read config")?.market.lockin_gas_estimate)
    }

    /// Estimate of gas for locking in any pending locks and submitting any pending proofs
    async fn estimate_gas_to_lock_pending(&self) -> Result<u64> {
        let mut gas = 0;
        for (_, order) in self.db.get_pending_lock_orders(0).await?.iter() {
            gas += self.estimate_gas_to_lock(order).await?;
        }
        Ok(gas)
    }

    /// Estimate of gas for fulfilling any orders either pending lock or locked
    async fn estimate_gas_to_fulfill_pending(&self) -> Result<u64> {
        let pending_fulfill_orders = self.db.get_orders_committed_to_fulfill_count().await?;
        Ok((pending_fulfill_orders as u64)
            * self.config.lock_all().context("Failed to read config")?.market.fulfill_gas_estimate)
    }

    /// Estimate the total gas tokens reserved to lock and fulfill all pending orders
    async fn gas_reserved(&self) -> Result<U256> {
        let gas_price = self.provider.get_gas_price().await.context("Failed to get gas price")?;
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

        let gas_reserved = self.gas_reserved().await?;

        tracing::debug!(
            "Available Balance = account_balance({}) - expected_future_gas({})",
            format_ether(balance),
            format_ether(gas_reserved)
        );

        Ok(balance - gas_reserved)
    }

    /// Return available stake balance.
    ///
    /// This is defined as the balance in staking tokens of the signer account minus any pending locked stake.
    async fn available_stake_balance(&self) -> Result<U256> {
        let balance = self.market.balance_of_stake(self.provider.default_signer_address()).await?;
        let pending_balance = self.pending_locked_stake().await?;
        Ok(balance - pending_balance)
    }
}

impl<P> RetryTask for OrderPicker<P>
where
    P: Provider<BoxTransport, Ethereum> + 'static + Clone + WalletProvider,
{
    fn spawn(&self) -> RetryRes {
        let picker_copy = self.clone();
        // Find existing Pricing orders and start processing them (resume)

        Box::pin(async move {
            tracing::info!("Starting order picking monitor");

            picker_copy.find_existing_orders().await.map_err(SupervisorErr::Fault)?;

            loop {
                let order_res = picker_copy
                    .db
                    .get_order_for_pricing()
                    .await
                    .map_err(|err| SupervisorErr::Recover(err.into()))?;

                if let Some((order_id, order)) = order_res {
                    let picker_clone = picker_copy.clone();
                    // TODO: We should consider having handles for these inner tasks
                    // but they are one-shots that self-clean up on the DB so maybe its fine?
                    tokio::spawn(async move {
                        if let Err(err) = picker_clone.price_order(order_id, &order).await {
                            picker_clone
                                .db
                                .set_order_failure(order_id, err.to_string())
                                .await
                                .expect("Failed to set order failure");
                            match err {
                                PriceOrderErr::OtherErr(err) => {
                                    tracing::error!("Pricing order failed: {order_id:x} {err:?}");
                                }
                                // Only warn on known / classified errors
                                _ => {
                                    tracing::warn!(
                                        "Pricing order soft failed: {order_id:x} {err:?}"
                                    );
                                }
                            }
                        }
                    });
                }
                // TODO: Configuration
                tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{db::SqliteDb, provers::MockProver, OrderStatus};
    use alloy::{
        network::EthereumWallet,
        node_bindings::{Anvil, AnvilInstance},
        primitives::{aliases::U96, Address, Bytes, B256},
        providers::{
            ext::AnvilApi,
            fillers::{
                BlobGasFiller, ChainIdFiller, FillProvider, GasFiller, JoinFill, NonceFiller,
                WalletFiller,
            },
            Identity, ProviderBuilder, RootProvider,
        },
        signers::local::PrivateKeySigner,
    };
    use boundless_market::contracts::{
        test_utils::{deploy_boundless_market, deploy_hit_points},
        Callback, Input, Offer, Predicate, PredicateType, ProofRequest, Requirements,
    };
    use chrono::Utc;
    use guest_assessor::ASSESSOR_GUEST_ID;
    use guest_util::{ECHO_ELF, ECHO_ID};
    use httpmock::prelude::*;
    use risc0_zkvm::sha::Digest;
    use tracing_test::traced_test;

    type TestProvider = FillProvider<
        JoinFill<
            JoinFill<
                Identity,
                JoinFill<GasFiller, JoinFill<BlobGasFiller, JoinFill<NonceFiller, ChainIdFiller>>>,
            >,
            WalletFiller<EthereumWallet>,
        >,
        RootProvider<BoxTransport>,
        BoxTransport,
        Ethereum,
    >;

    /// Reusable context for testing the order picker
    struct TestCtx<P> {
        pub anvil: AnvilInstance,
        pub picker: OrderPicker<P>,
        pub boundless_market: BoundlessMarketService<BoxTransport, Arc<P>>,
        pub image_server: MockServer,
        pub db: DbObj,
        pub provider: Arc<P>,
    }

    impl TestCtx<TestProvider> {
        pub fn builder() -> TestCtxBuilder {
            TestCtxBuilder::default()
        }

        pub fn image_uri(&self) -> String {
            format!("http://{}/image", self.image_server.address())
        }

        pub fn signer(&self, index: usize) -> PrivateKeySigner {
            self.anvil.keys()[index].clone().into()
        }

        pub async fn next_order(
            &self,
            min_price: U256,
            max_price: U256,
            lock_stake: U256,
        ) -> (U256, Order) {
            let image_id = Digest::from(ECHO_ID);
            let order_index = self.boundless_market.index_from_nonce().await.unwrap();
            (
                U256::from(order_index),
                Order {
                    status: OrderStatus::Pricing,
                    updated_at: Utc::now(),
                    request: ProofRequest::new(
                        order_index,
                        &self.provider.default_signer_address(),
                        Requirements {
                            imageId: <[u8; 32]>::from(image_id).into(),
                            predicate: Predicate {
                                predicateType: PredicateType::PrefixMatch,
                                data: Default::default(),
                            },
                            callback: Callback::default(),
                        },
                        self.image_uri(),
                        Input::builder()
                            .write_slice(&[0x41, 0x41, 0x41, 0x41])
                            .build_inline()
                            .unwrap(),
                        Offer {
                            minPrice: min_price,
                            maxPrice: max_price,
                            biddingStart: 0,
                            timeout: 100,
                            lockTimeout: 100,
                            rampUpPeriod: 1,
                            lockStake: lock_stake,
                        },
                    ),
                    target_block: None,
                    image_id: None,
                    input_id: None,
                    proof_id: None,
                    expire_block: None,
                    client_sig: Bytes::new(),
                    lock_price: None,
                    error_msg: None,
                },
            )
        }
    }

    #[derive(Default)]
    struct TestCtxBuilder {
        initial_signer_eth: Option<i32>,
        initial_hp: Option<U256>,
        config: Option<ConfigLock>,
    }

    impl TestCtxBuilder {
        pub fn with_initial_signer_eth(self, eth: i32) -> Self {
            Self { initial_signer_eth: Some(eth), ..self }
        }
        pub fn with_initial_hp(self, hp: U256) -> Self {
            assert!(hp < U256::from(U96::MAX), "Cannot have more than 2^96 hit points");
            Self { initial_hp: Some(hp), ..self }
        }
        pub fn with_config(self, config: ConfigLock) -> Self {
            Self { config: Some(config), ..self }
        }
        pub async fn build(self) -> TestCtx<TestProvider> {
            let anvil = Anvil::new()
                .args(["--balance", &format!("{}", self.initial_signer_eth.unwrap_or(10000))])
                .spawn();
            let signer: PrivateKeySigner = anvil.keys()[0].clone().into();
            let provider = Arc::new(
                ProviderBuilder::new()
                    .with_recommended_fillers()
                    .wallet(EthereumWallet::from(signer.clone()))
                    .on_builtin(&anvil.endpoint())
                    .await
                    .unwrap(),
            );

            provider.anvil_mine(Some(U256::from(4)), Some(U256::from(2))).await.unwrap();

            let hp_contract = deploy_hit_points(&signer, provider.clone()).await.unwrap();
            let market_address = deploy_boundless_market(
                &signer,
                provider.clone(),
                Address::ZERO,
                hp_contract,
                Digest::from(ASSESSOR_GUEST_ID),
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

            let image_server = MockServer::start();
            let _get_mock = image_server.mock(|when, then| {
                when.method(GET).path("/image");
                then.status(200).body(ECHO_ELF);
            });

            let db: DbObj = Arc::new(SqliteDb::new("sqlite::memory:").await.unwrap());
            let config = self.config.unwrap_or_default();
            let prover: ProverObj = Arc::new(MockProver::default());
            let chain_monitor = Arc::new(ChainMonitorService::new(provider.clone()).await.unwrap());
            tokio::spawn(chain_monitor.spawn());

            let picker = OrderPicker::new(
                db.clone(),
                config,
                prover,
                2,
                market_address,
                provider.clone(),
                chain_monitor,
            );

            TestCtx { anvil, picker, boundless_market, image_server, db, provider }
        }
    }

    #[tokio::test]
    #[traced_test]
    async fn price_order() {
        let config = ConfigLock::default();
        {
            config.load_write().unwrap().market.mcycle_price = "0.0000001".into();
        }
        let ctx = TestCtx::builder().with_config(config).build().await;

        let min_price = 200000000000u64;
        let max_price = 400000000000u64;

        let (order_id, order) =
            ctx.next_order(U256::from(min_price), U256::from(max_price), U256::from(0)).await;

        let _request_id =
            ctx.boundless_market.submit_request(&order.request, &ctx.signer(0)).await.unwrap();

        ctx.db.add_order(order_id, order.clone()).await.unwrap();
        ctx.picker.price_order(order_id, &order).await.unwrap();

        let db_order = ctx.db.get_order(order_id).await.unwrap().unwrap();
        assert_eq!(db_order.status, OrderStatus::Locking);
        assert_eq!(db_order.target_block, Some(order.request.offer.biddingStart));
    }

    #[tokio::test]
    #[traced_test]
    async fn skip_bad_predicate() {
        let config = ConfigLock::default();
        {
            config.load_write().unwrap().market.mcycle_price = "0.0000001".into();
        }
        let ctx = TestCtx::builder().with_config(config).build().await;

        let min_price = 200000000000u64;
        let max_price = 400000000000u64;

        let (order_id, mut order) =
            ctx.next_order(U256::from(min_price), U256::from(max_price), U256::from(0)).await;

        // set a bad predicate
        order.request.requirements.predicate =
            Predicate { predicateType: PredicateType::DigestMatch, data: B256::ZERO.into() };

        let _request_id =
            ctx.boundless_market.submit_request(&order.request, &ctx.signer(0)).await.unwrap();

        ctx.db.add_order(order_id, order.clone()).await.unwrap();
        ctx.picker.price_order(order_id, &order).await.unwrap();

        let db_order = ctx.db.get_order(order_id).await.unwrap().unwrap();
        assert_eq!(db_order.status, OrderStatus::Skipped);

        assert!(logs_contain("predicate check failed, skipping"));
    }

    #[tokio::test]
    #[traced_test]
    async fn skip_unallowed_addr() {
        let config = ConfigLock::default();
        {
            config.load_write().unwrap().market.mcycle_price = "0.0000001".into();
            config.load_write().unwrap().market.allow_client_addresses = Some(vec![Address::ZERO]);
        }
        let ctx = TestCtx::builder().with_config(config).build().await;

        let min_price = 200000000000u64;
        let max_price = 400000000000u64;

        let (order_id, order) =
            ctx.next_order(U256::from(min_price), U256::from(max_price), U256::from(0)).await;

        let _request_id =
            ctx.boundless_market.submit_request(&order.request, &ctx.signer(0)).await.unwrap();

        ctx.db.add_order(order_id, order.clone()).await.unwrap();
        ctx.picker.price_order(order_id, &order).await.unwrap();

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
        let ctx = TestCtx::builder().with_config(config).build().await;

        let min_price = 200000000000u64;
        let max_price = 400000000000u64;

        let (order_id, order) =
            ctx.next_order(U256::from(min_price), U256::from(max_price), U256::from(0)).await;

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
        assert_eq!(db_order.target_block, Some(order.request.offer.biddingStart));
    }

    // TODO: Test
    // need to test the non-ASAP path for pricing, aka picking a block ahead in time to make sure
    // that price calculator is working correctly.

    #[tokio::test]
    #[traced_test]
    async fn pending_locked_stake() {
        let lockin_stake = U256::from(10);

        let config = ConfigLock::default();
        {
            config.load_write().unwrap().market.mcycle_price = "0.0000001".into();
            config.load_write().unwrap().market.max_stake = "10".into();
        }

        let ctx =
            TestCtx::builder().with_config(config).with_initial_hp(U256::from(100)).build().await;
        assert_eq!(ctx.picker.pending_locked_stake().await.unwrap(), U256::ZERO);

        let (order_id, order) = ctx
            .next_order(U256::from(200000000000u64), U256::from(400000000000u64), lockin_stake)
            .await;

        ctx.db.add_order(order_id, order.clone()).await.unwrap();
        ctx.picker.price_order(order_id, &order).await.unwrap();
        // order is pending lock so stake is counted
        assert_eq!(ctx.picker.pending_locked_stake().await.unwrap(), lockin_stake);

        ctx.db.set_order_lock(order_id, 2, 100).await.unwrap();
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

        let ctx = TestCtx::builder().with_config(config).build().await;
        assert_eq!(ctx.picker.pending_locked_stake().await.unwrap(), U256::ZERO);

        let (order_id, order) = ctx
            .next_order(U256::from(200000000000u64), U256::from(400000000000u64), U256::ZERO)
            .await;
        assert_eq!(ctx.picker.estimate_gas_to_lock(&order).await.unwrap(), lockin_gas);

        ctx.db.add_order(order_id, order.clone()).await.unwrap();
        ctx.picker.price_order(order_id, &order).await.unwrap();

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

        let ctx = TestCtx::builder().with_config(config).build().await;
        assert_eq!(ctx.picker.pending_locked_stake().await.unwrap(), U256::ZERO);

        let (_, order) = ctx
            .next_order(U256::from(200000000000u64), U256::from(400000000000u64), U256::ZERO)
            .await;
        ctx.db.add_order(U256::from(0), order.clone()).await.unwrap();
        ctx.picker.price_order(U256::from(0), &order).await.unwrap();

        assert_eq!(ctx.picker.estimate_gas_to_fulfill_pending().await.unwrap(), fulfill_gas);

        // add another order
        let (_, order) = ctx
            .next_order(U256::from(200000000000u64), U256::from(400000000000u64), U256::ZERO)
            .await;
        ctx.db.add_order(U256::from(1), order.clone()).await.unwrap();
        ctx.picker.price_order(U256::from(1), &order).await.unwrap();

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

        let ctx = TestCtx::builder().with_config(config).build().await;
        assert_eq!(ctx.picker.pending_locked_stake().await.unwrap(), U256::ZERO);

        let (order_id, order) = ctx
            .next_order(U256::from(200000000000u64), U256::from(400000000000u64), U256::ZERO)
            .await;
        ctx.db.add_order(order_id, order.clone()).await.unwrap();
        ctx.picker.price_order(order_id, &order).await.unwrap();

        let gas_price = ctx.provider.get_gas_price().await.unwrap();
        assert_eq!(
            ctx.picker.gas_reserved().await.unwrap(),
            U256::from(gas_price) * U256::from(fulfill_gas + lockin_gas)
        );
        // lock the order
        ctx.db.set_order_lock(order_id, 2, 100).await.unwrap();
        // only fulfillment gas now reserved
        assert_eq!(
            ctx.picker.gas_reserved().await.unwrap(),
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

        let ctx = TestCtx::builder()
            .with_initial_signer_eth(signer_inital_balance_eth)
            .with_initial_hp(lockin_stake)
            .with_config(config)
            .build()
            .await;
        let (_, order) = ctx
            .next_order(U256::from(200000000000u64), U256::from(400000000000u64), U256::from(100))
            .await;

        let orders = std::iter::repeat(order).take(2).collect::<Vec<_>>();

        for (order_id, order) in orders.iter().enumerate() {
            ctx.db.add_order(U256::from(order_id), order.clone()).await.unwrap();
            ctx.picker.price_order(U256::from(order_id), order).await.unwrap();
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
        let lockin_stake = U256::from(10);

        let ctx =
            TestCtx::builder().with_config(config).with_initial_hp(lockin_stake).build().await;
        let (_, order) = ctx
            .next_order(U256::from(200000000000u64), U256::from(400000000000u64), lockin_stake)
            .await;

        let order_id = U256::from(0);
        ctx.db.add_order(U256::from(order_id), order.clone()).await.unwrap();
        ctx.picker.price_order(U256::from(order_id), &order).await.unwrap();

        assert_eq!(ctx.db.get_order(order_id).await.unwrap().unwrap().status, OrderStatus::Skipped);
        assert!(logs_contain("journal larger than set limit"));
    }
}
