// Copyright (c) 2025 RISC Zero, Inc.
//
// All rights reserved.

use crate::{
    chain_monitor::ChainMonitorService,
    config::ConfigLock,
    db::DbObj,
    task::{RetryRes, RetryTask, SupervisorErr},
    Order, OrderStatus,
};
use alloy::{
    network::Ethereum,
    primitives::{Address, U256},
    providers::{Provider, WalletProvider},
    transports::BoxTransport,
};
use anyhow::{Context, Result};
use boundless_market::contracts::{
    boundless_market::{BoundlessMarketService, MarketError},
    ProofStatus,
};
use std::{sync::Arc, time::Duration};
use thiserror::Error;

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

#[derive(Clone)]
pub struct OrderMonitor<P> {
    db: DbObj,
    chain_monitor: Arc<ChainMonitorService<P>>,
    block_time: u64,
    config: ConfigLock,
    market: BoundlessMarketService<BoxTransport, Arc<P>>,
}

impl<P> OrderMonitor<P>
where
    P: Provider<BoxTransport, Ethereum> + WalletProvider + 'static + Clone,
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

        Ok(Self { db, chain_monitor, block_time, config, market })
    }

    async fn lock_order(&self, order_id: U256, order: &Order) -> Result<(), LockOrderErr> {
        if order.status != OrderStatus::Locking {
            return Err(LockOrderErr::InvalidStatus(order.status));
        }

        let order_status = self
            .market
            .get_status(order_id, Some(order.request.expires_at()))
            .await
            .context("Failed to get order status")?;
        if order_status != ProofStatus::Unknown {
            tracing::warn!("Order {order_id:x} not open: {order_status:?}, skipping");
            // TODO: fetch some chain data to find out who / and for how much the order
            // was locked in at
            return Err(LockOrderErr::AlreadyLocked);
        }

        let conf_priority_gas = {
            let conf = self.config.lock_all().context("Failed to lock config")?;
            conf.market.lockin_priority_gas
        };

        tracing::info!("Locking order: {order_id:x} for stake: {}", order.request.offer.lockStake);
        let lock_block = self
            .market
            .lock_request(&order.request, &order.client_sig, conf_priority_gas)
            .await
            .map_err(LockOrderErr::OrderLockedInBlock)?;

        let lock_price = self
            .market
            .price_at_block(&order.request.offer, lock_block)
            .context("Failed to calculate lock price")?;

        self.db.set_proving_status(order_id, lock_price).await.with_context(|| {
            format!(
                "FATAL STAKE AT RISK: {order_id:x} failed to move from locking -> proving status"
            )
        })?;

        Ok(())
    }

    async fn lock_orders(&self, current_block: u64, orders: Vec<(U256, Order)>) -> Result<u64> {
        let mut order_count = 0;
        for (order_id, order) in orders.iter() {
            match self.lock_order(*order_id, order).await {
                Ok(_) => tracing::info!("Locked order: {order_id:x}"),
                Err(ref err) => {
                    match err {
                        LockOrderErr::OtherErr(err) => {
                            tracing::error!("Failed to lock order: {order_id:x} {err:?}");
                        }
                        // Only warn on known / classified errors
                        _ => {
                            tracing::warn!("Soft failed to lock order: {order_id:x} {err:?}");
                        }
                    }
                    if let Err(err) = self.db.set_order_failure(*order_id, format!("{err:?}")).await
                    {
                        tracing::error!(
                            "Failed to set DB failure state for order: {order_id:x}, {err:?}"
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
        let order_count = if let Some(last_monitor_block) = opt_last_block {
            let current_block = self.chain_monitor.current_block_number().await?;

            tracing::debug!(
                "Search {last_monitor_block} - {current_block} blocks for lock pending orders..."
            );

            let orders = self
                .db
                .get_pending_lock_orders(current_block)
                .await
                .context("Failed to find pending lock orders")?;

            self.lock_orders(current_block, orders).await.context("Failed to lock orders")?
        } else {
            0
        };

        Ok(order_count)
    }

    // TODO:
    // need to call set_failed() correctly whenever a order triggers a hard failure
    pub async fn start_monitor(&self, block_limit: Option<u64>) -> Result<()> {
        self.back_scan_locks().await?;

        // TODO: Move to websocket subscriptions
        let mut last_block = 0;
        let mut first_block = 0;
        loop {
            let current_block = self.chain_monitor.current_block_number().await?;

            if current_block != last_block {
                last_block = current_block;
                if first_block == 0 {
                    first_block = current_block;
                }

                let orders = self
                    .db
                    .get_pending_lock_orders(current_block)
                    .await
                    .context("Failed to find pending lock orders")?;

                self.lock_orders(current_block, orders).await.context("Failed to lock orders")?;

                // Bailout if configured to only run for N blocks
                if let Some(block_lim) = block_limit {
                    if block_lim > current_block - first_block {
                        return Ok(());
                    }
                }
            }

            // Attempt to wait 1/2 a block time to catch each new block
            tokio::time::sleep(tokio::time::Duration::from_secs(self.block_time / 2)).await
        }
    }
}

impl<P> RetryTask for OrderMonitor<P>
where
    P: Provider<BoxTransport, Ethereum> + WalletProvider + 'static + Clone,
{
    fn spawn(&self) -> RetryRes {
        let monitor_clone = self.clone();
        Box::pin(async move {
            tracing::info!("Starting order monitor");
            monitor_clone.start_monitor(None).await.map_err(SupervisorErr::Recover)?;
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::SqliteDb;
    use alloy::{
        network::EthereumWallet,
        node_bindings::Anvil,
        primitives::{Address, B256, U256},
        providers::{ext::AnvilApi, ProviderBuilder},
        signers::local::PrivateKeySigner,
    };
    use boundless_market::contracts::{
        test_utils::{deploy_boundless_market, deploy_hit_points}, Callback, Input, InputType, Offer, Predicate, PredicateType, ProofRequest, Requirements
    };
    use chrono::Utc;
    use guest_assessor::ASSESSOR_GUEST_ID;
    use risc0_zkvm::sha::Digest;
    use tracing_test::traced_test;

    #[tokio::test]
    #[traced_test]
    async fn back_scan_lock() {
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

        let hit_points = deploy_hit_points(&signer, provider.clone()).await.unwrap();

        let market_address = deploy_boundless_market(
            &signer,
            provider.clone(),
            Address::ZERO,
            hit_points,
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

        let block_time = 2;
        let min_price = 1;
        let max_price = 2;

        let request = ProofRequest::new(
            1,
            &signer.address(),
            Requirements {
                imageId: B256::ZERO,
                predicate: Predicate {
                    predicateType: PredicateType::PrefixMatch,
                    data: Default::default(),
                },
                callback: Callback::default(),
            },
            "http://risczero.com/image",
            Input { inputType: InputType::Inline, data: Default::default() },
            Offer {
                minPrice: U256::from(min_price),
                maxPrice: U256::from(max_price),
                biddingStart: 0,
                rampUpPeriod: 1,
                timeout: 100,
                lockTimeout: 100,
                lockStake: U256::from(0),
            },
        );
        let order_id = U256::from(request.id);
        tracing::info!("addr: {} ID: {:x}", signer.address(), request.id);

        // let client_sig = boundless_market.eip721_signature(&request, &signer).await.unwrap();
        let chain_id = provider.get_chain_id().await.unwrap();
        let client_sig =
            request.sign_request(&signer, market_address, chain_id).await.unwrap().as_bytes();

        let order = Order {
            status: OrderStatus::Locking,
            updated_at: Utc::now(),
            target_block: Some(0),
            request,
            image_id: None,
            input_id: None,
            proof_id: None,
            expire_block: None,
            client_sig: client_sig.into(),
            lock_price: None,
            error_msg: None,
        };
        let request_id = boundless_market.submit_request(&order.request, &signer).await.unwrap();
        assert_eq!(request_id, order_id);

        provider.anvil_mine(Some(U256::from(2)), Some(U256::from(block_time))).await.unwrap();

        db.add_order(order_id, order).await.unwrap();
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

        let order = db.get_order(order_id).await.unwrap().unwrap();
        if let OrderStatus::Failed = order.status {
            let err = order.error_msg.expect("Missing error message for failed order");
            panic!("order failed: {err}");
        }
        assert!(matches!(order.status, OrderStatus::Locked));
    }

    #[tokio::test]
    #[traced_test]
    async fn monitor_block() {
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

        let hit_points = deploy_hit_points(&signer, provider.clone()).await.unwrap();

        let market_address = deploy_boundless_market(
            &signer,
            provider.clone(),
            Address::ZERO,
            hit_points,
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

        let block_time = 2;
        let min_price = 1;
        let max_price = 2;

        let request = ProofRequest::new(
            boundless_market.index_from_nonce().await.unwrap(),
            &signer.address(),
            Requirements {
                imageId: B256::ZERO,
                predicate: Predicate {
                    predicateType: PredicateType::PrefixMatch,
                    data: Default::default(),
                },
                callback: Callback::default(),
            },
            "http://risczero.com/image",
            Input { inputType: InputType::Inline, data: Default::default() },
            Offer {
                minPrice: U256::from(min_price),
                maxPrice: U256::from(max_price),
                biddingStart: 0,
                rampUpPeriod: 1,
                timeout: 100,
                lockTimeout: 100,
                lockStake: U256::from(0),
            },
        );
        let order_id = U256::from(request.id);
        tracing::info!("addr: {} ID: {:x}", signer.address(), order_id);

        let chain_id = provider.get_chain_id().await.unwrap();
        let client_sig = request
            .sign_request(&signer, market_address, chain_id)
            .await
            .unwrap()
            .as_bytes()
            .into();
        let order = Order {
            status: OrderStatus::Locking,
            updated_at: Utc::now(),
            target_block: Some(0),
            request,
            image_id: None,
            input_id: None,
            proof_id: None,
            expire_block: None,
            client_sig,
            lock_price: None,
            error_msg: None,
        };

        let _request_id = boundless_market.submit_request(&order.request, &signer).await.unwrap();

        db.add_order(order_id, order).await.unwrap();

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

        monitor.start_monitor(Some(4)).await.unwrap();

        let order = db.get_order(order_id).await.unwrap().unwrap();
        assert_eq!(order.status, OrderStatus::Locked);
    }
}
