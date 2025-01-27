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

        let current_balance = self
            .provider
            .get_balance(self.provider.default_signer_address())
            .await
            .context("Failed to get current wallet balance")?;

        // Check that we have the funds to handle this
        // TODO: with two parallel price_orders() running we could hit an issue
        // were we over commit on stake. Need to probably sync the stake
        if lockin_stake >= current_balance {
            tracing::warn!("Stake is higher than current balance on order {order_id:x}");
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

        // Validate the predicates:
        let journal = self
            .prover
            .get_preflight_journal(&proof_res.id)
            .await
            .context("Failed to fetch preflight journal")?
            .context("Failed to find preflight journal")?;

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
    use crate::{
        db::SqliteDb,
        provers::{encode_input, MockProver},
        OrderStatus,
    };
    use alloy::{
        network::EthereumWallet,
        node_bindings::Anvil,
        primitives::{Address, Bytes, B256, U256},
        providers::{ext::AnvilApi, ProviderBuilder},
        signers::local::PrivateKeySigner,
    };
    use boundless_market::{
        contracts::{
            test_utils::deploy_boundless_market, Input, InputType, Offer, Predicate, PredicateType,
            ProofRequest, Requirements,
        },
        input::InputEnv,
    };
    use chrono::Utc;
    use guest_assessor::ASSESSOR_GUEST_ID;
    use guest_util::{ECHO_ELF, ECHO_ID};
    use httpmock::prelude::*;
    use risc0_zkvm::sha::Digest;
    use tracing_test::traced_test;

    // TODO: We need to make a testing harness to run lots of different
    // orders + configs through the system of price_order()
    // so we need a way to quickly define the parameters or spin up the deps
    #[tokio::test]
    #[traced_test]
    async fn price_order() {
        let anvil = Anvil::new().spawn();
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

        let market_address = deploy_boundless_market(
            &signer,
            provider.clone(),
            Address::ZERO,
            Address::ZERO,
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

        let db: DbObj = Arc::new(SqliteDb::new("sqlite::memory:").await.unwrap());
        let config = ConfigLock::default();

        {
            config.load_write().unwrap().market.mcycle_price = "0.0000001".into();
        }

        let prover: ProverObj = Arc::new(MockProver::default());
        let image_id = Digest::from(ECHO_ID);
        let input_buf =
            InputEnv::new().write(&vec![0x41, 0x41, 0x41, 0x41]).unwrap().encode().unwrap();

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

        let server = MockServer::start();
        let get_mock = server.mock(|when, then| {
            when.method(GET).path("/image");
            then.status(200).body(ECHO_ELF);
        });
        let image_uri = format!("http://{}/image", server.address());

        let min_price = 200000000000u64;
        let max_price = 400000000000u64;

        let order_id = U256::ZERO;
        let order = Order {
            status: OrderStatus::Pricing,
            updated_at: Utc::now(),
            request: ProofRequest::new(
                boundless_market.index_from_nonce().await.unwrap(),
                &signer.address(),
                Requirements {
                    imageId: <[u8; 32]>::from(image_id).into(),
                    predicate: Predicate {
                        predicateType: PredicateType::PrefixMatch,
                        data: Default::default(),
                    },
                },
                &image_uri,
                Input { inputType: InputType::Inline, data: input_buf.into() },
                Offer {
                    minPrice: U256::from(min_price),
                    maxPrice: U256::from(max_price),
                    biddingStart: 0,
                    timeout: 100,
                    rampUpPeriod: 1,
                    lockStake: U256::from(0),
                },
            ),
            target_block: None,
            image_id: None,
            input_id: None,
            proof_id: None,
            expire_block: None,
            path: None,
            client_sig: Bytes::new(),
            lock_price: None,
            error_msg: None,
        };

        let _request_id = boundless_market.submit_request(&order.request, &signer).await.unwrap();

        db.add_order(order_id, order.clone()).await.unwrap();
        picker.price_order(order_id, &order).await.unwrap();

        let db_order = db.get_order(order_id).await.unwrap().unwrap();
        assert_eq!(db_order.status, OrderStatus::Locking);
        assert_eq!(db_order.target_block, Some(order.request.offer.biddingStart));

        get_mock.assert();
    }

    #[tokio::test]
    #[traced_test]
    async fn skip_bad_predicate() {
        let anvil = Anvil::new().spawn();
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

        let market_address = deploy_boundless_market(
            &signer,
            provider.clone(),
            Address::ZERO,
            Address::ZERO,
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

        let db: DbObj = Arc::new(SqliteDb::new("sqlite::memory:").await.unwrap());
        let config = ConfigLock::default();

        {
            config.load_write().unwrap().market.mcycle_price = "0.0000001".into();
        }

        let prover: ProverObj = Arc::new(MockProver::default());
        let image_id = Digest::from(ECHO_ID);
        let input_buf =
            InputEnv::new().write(&vec![0x41, 0x41, 0x41, 0x41]).unwrap().encode().unwrap();

        let chain_monitor = Arc::new(ChainMonitorService::new(provider.clone()).await.unwrap());
        tokio::spawn(chain_monitor.spawn());

        let picker = OrderPicker::new(
            db.clone(),
            config,
            prover,
            2,
            market_address,
            provider,
            chain_monitor,
        );

        let server = MockServer::start();
        let get_mock = server.mock(|when, then| {
            when.method(GET).path("/image");
            then.status(200).body(ECHO_ELF);
        });
        let image_uri = format!("http://{}/image", server.address());

        let min_price = 200000000000u64;
        let max_price = 400000000000u64;

        let order_id = U256::ZERO;
        let order = Order {
            status: OrderStatus::Pricing,
            updated_at: Utc::now(),
            target_block: None,
            request: ProofRequest::new(
                boundless_market.index_from_nonce().await.unwrap(),
                &signer.address(),
                Requirements {
                    imageId: <[u8; 32]>::from(image_id).into(),
                    predicate: Predicate {
                        predicateType: PredicateType::DigestMatch,
                        data: B256::ZERO.into(),
                    },
                },
                &image_uri,
                Input { inputType: InputType::Inline, data: input_buf.into() },
                Offer {
                    minPrice: U256::from(min_price),
                    maxPrice: U256::from(max_price),
                    biddingStart: 0,
                    timeout: 100,
                    rampUpPeriod: 1,
                    lockStake: U256::from(0),
                },
            ),
            image_id: None,
            input_id: None,
            proof_id: None,
            expire_block: None,
            path: None,
            client_sig: Bytes::new(),
            lock_price: None,
            error_msg: None,
        };

        let _request_id = boundless_market.submit_request(&order.request, &signer).await.unwrap();

        db.add_order(order_id, order.clone()).await.unwrap();
        picker.price_order(order_id, &order).await.unwrap();

        let db_order = db.get_order(order_id).await.unwrap().unwrap();
        assert_eq!(db_order.status, OrderStatus::Skipped);

        assert!(logs_contain("predicate check failed, skipping"));

        get_mock.assert();
    }

    #[tokio::test]
    #[traced_test]
    async fn skip_unallowed_addr() {
        let anvil = Anvil::new().spawn();
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

        let market_address = deploy_boundless_market(
            &signer,
            provider.clone(),
            Address::ZERO,
            Address::ZERO,
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

        let db: DbObj = Arc::new(SqliteDb::new("sqlite::memory:").await.unwrap());
        let config = ConfigLock::default();

        {
            config.load_write().unwrap().market.allow_client_addresses = Some(vec![Address::ZERO]);
        }

        let prover: ProverObj = Arc::new(MockProver::default());
        let image_id = Digest::from(ECHO_ID);
        let input_buf = encode_input(&vec![0x41, 0x41, 0x41, 0x41]).unwrap();

        let chain_monitor = Arc::new(ChainMonitorService::new(provider.clone()).await.unwrap());
        tokio::spawn(chain_monitor.spawn());
        let picker = OrderPicker::new(
            db.clone(),
            config,
            prover,
            2,
            market_address,
            provider,
            chain_monitor,
        );

        let order_id = U256::from(boundless_market.request_id_from_nonce().await.unwrap());
        let min_price = 200000000000u64;
        let max_price = 400000000000u64;

        let order = Order {
            status: OrderStatus::Pricing,
            updated_at: Utc::now(),
            target_block: None,
            request: ProofRequest::new(
                boundless_market.index_from_nonce().await.unwrap(),
                &signer.address(),
                Requirements {
                    imageId: <[u8; 32]>::from(image_id).into(),
                    predicate: Predicate {
                        predicateType: PredicateType::DigestMatch,
                        data: B256::ZERO.into(),
                    },
                },
                "",
                Input { inputType: InputType::Inline, data: input_buf.into() },
                Offer {
                    minPrice: U256::from(min_price),
                    maxPrice: U256::from(max_price),
                    biddingStart: 0,
                    timeout: 100,
                    rampUpPeriod: 1,
                    lockStake: U256::from(0),
                },
            ),
            image_id: None,
            input_id: None,
            proof_id: None,
            expire_block: None,
            path: None,
            client_sig: Bytes::new(),
            lock_price: None,
            error_msg: None,
        };

        let _request_id = boundless_market.submit_request(&order.request, &signer).await.unwrap();

        db.add_order(order_id, order.clone()).await.unwrap();
        picker.price_order(order_id, &order).await.unwrap();

        let db_order = db.get_order(order_id).await.unwrap().unwrap();
        assert_eq!(db_order.status, OrderStatus::Skipped);

        assert!(logs_contain("because it is not in allowed addrs"));
    }

    #[tokio::test]
    #[traced_test]
    async fn resume_order_pricing() {
        let anvil = Anvil::new().spawn();
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
        let market_address = deploy_boundless_market(
            &signer,
            provider.clone(),
            Address::ZERO,
            Address::ZERO,
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

        let db: DbObj = Arc::new(SqliteDb::new("sqlite::memory:").await.unwrap());
        let config = ConfigLock::default();

        {
            config.load_write().unwrap().market.mcycle_price = "0.0000001".into();
        }

        let prover: ProverObj = Arc::new(MockProver::default());
        let image_id = Digest::from(ECHO_ID);
        let input_buf =
            InputEnv::new().write(&vec![0x41, 0x41, 0x41, 0x41]).unwrap().encode().unwrap();

        let chain_monitor = Arc::new(ChainMonitorService::new(provider.clone()).await.unwrap());
        tokio::spawn(chain_monitor.spawn());
        let picker = OrderPicker::new(
            db.clone(),
            config,
            prover,
            2,
            market_address,
            provider,
            chain_monitor,
        );

        let server = MockServer::start();
        let get_mock = server.mock(|when, then| {
            when.method(GET).path("/image");
            then.status(200).body(ECHO_ELF);
        });
        let image_uri = format!("http://{}/image", server.address());

        let min_price = 200000000000u64;
        let max_price = 400000000000u64;

        let order_id = U256::ZERO;
        let order = Order {
            status: OrderStatus::Pricing,
            updated_at: Utc::now(),
            request: ProofRequest::new(
                boundless_market.index_from_nonce().await.unwrap(),
                &signer.address(),
                Requirements {
                    imageId: <[u8; 32]>::from(image_id).into(),
                    predicate: Predicate {
                        predicateType: PredicateType::PrefixMatch,
                        data: Default::default(),
                    },
                },
                &image_uri,
                Input { inputType: InputType::Inline, data: input_buf.into() },
                Offer {
                    minPrice: U256::from(min_price),
                    maxPrice: U256::from(max_price),
                    biddingStart: 0,
                    timeout: 100,
                    rampUpPeriod: 1,
                    lockStake: U256::from(0),
                },
            ),
            target_block: None,
            image_id: None,
            input_id: None,
            proof_id: None,
            expire_block: None,
            path: None,
            client_sig: Bytes::new(),
            lock_price: None,
            error_msg: None,
        };

        let _request_id = boundless_market.submit_request(&order.request, &signer).await.unwrap();
        db.add_order(order_id, order.clone()).await.unwrap();

        picker.find_existing_orders().await.unwrap();

        assert!(logs_contain("Found 1 orders currently pricing to resume"));

        // Try and wait for the order to complete pricing
        for _ in 0..4 {
            let db_order = db.get_order(order_id).await.unwrap().unwrap();
            if db_order.status != OrderStatus::Pricing {
                break;
            }
            tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
        }

        let db_order = db.get_order(order_id).await.unwrap().unwrap();
        assert_eq!(db_order.status, OrderStatus::Locking);
        assert_eq!(db_order.target_block, Some(order.request.offer.biddingStart));

        get_mock.assert();
    }

    // TODO: Test
    // need to test the non-ASAP path for pricing, aka picking a block ahead in time to make sure
    // that price calculator is working correctly.
}
