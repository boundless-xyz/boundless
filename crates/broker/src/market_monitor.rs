// Copyright (c) 2024 RISC Zero, Inc.
//
// All rights reserved.

use std::{marker::PhantomData, sync::Arc};

use alloy::{
    network::Ethereum, primitives::Address, providers::Provider, rpc::types::Filter,
    sol_types::SolEvent, transports::Transport,
};
use anyhow::{Context, Result};
use boundless_market::contracts::{proof_market::ProofMarketService, IProofMarket, ProofStatus};
use futures_util::StreamExt;

use crate::{
    task::{RetryRes, RetryTask, SupervisorErr},
    DbObj, Order,
};

const BLOCK_TIME_SAMPLE_SIZE: u64 = 10;

pub struct MarketMonitor<T, P> {
    lookback_blocks: u64,
    market_addr: Address,
    provider: Arc<P>,
    db: DbObj,
    _phantom_t: PhantomData<T>,
}

impl<T, P> MarketMonitor<T, P>
where
    T: Transport + Clone,
    P: Provider<T, Ethereum> + 'static + Clone,
{
    pub fn new(lookback_blocks: u64, market_addr: Address, provider: Arc<P>, db: DbObj) -> Self {
        Self { lookback_blocks, market_addr, provider, db, _phantom_t: Default::default() }
    }

    /// Queries chain history to sample for the median block time
    pub async fn get_block_time(&self) -> Result<u64> {
        let current_block =
            self.provider.get_block_number().await.context("failed to get current block")?;

        let mut timestamps = vec![];
        for i in current_block - BLOCK_TIME_SAMPLE_SIZE..current_block {
            let block = self
                .provider
                .get_block_by_number(i.into(), false)
                .await
                .with_context(|| format!("Failed get block {i}"))?
                .with_context(|| format!("Missing block {i}"))?;

            timestamps.push(block.header.timestamp);
        }

        let mut block_times =
            timestamps.windows(2).map(|elm| elm[1] - elm[0]).collect::<Vec<u64>>();
        block_times.sort();

        Ok(block_times[block_times.len() / 2])
    }

    async fn find_open_orders(
        lookback_blocks: u64,
        market_addr: Address,
        provider: Arc<P>,
        db: DbObj,
    ) -> Result<u64> {
        let current_block =
            provider.get_block_number().await.context("Failed to get current block numb")?;

        let start_block = current_block.saturating_sub(lookback_blocks);

        tracing::info!("Searching for existing open orders: {start_block} - {current_block}");

        let market = ProofMarketService::new(market_addr, provider.clone(), Address::ZERO);
        // let event: Event<_, _, IProofMarket::RequestSubmitted, _> = Event::new(
        //     provider.clone(),
        //     Filter::new().from_block(start_block).address(market_addr),
        // );

        // let logs = event.query().await.context("Failed to query RequestSubmitted events")?;

        let filter = Filter::new()
            .event_signature(IProofMarket::RequestSubmitted::SIGNATURE_HASH)
            .from_block(start_block)
            .address(market_addr);

        // TODO: This could probably be cleaned up but the alloy examples
        // don't have a lot of clean log decoding samples, and the Event::query()
        // interface would randomly fail for me?
        let logs = provider.get_logs(&filter).await?;
        let decoded_logs = logs.iter().filter_map(|log| {
            match log.log_decode::<IProofMarket::RequestSubmitted>() {
                Ok(res) => Some(res),
                Err(err) => {
                    tracing::error!("Failed to decode RequestSubmitted log: {err:?}");
                    None
                }
            }
        });

        tracing::debug!("Found {} possible in the past {} blocks", logs.len(), lookback_blocks);
        let mut order_count = 0;
        for log in decoded_logs {
            let event = &log.inner.data;
            let order_exists = match db.order_exists(event.request.id).await {
                Ok(val) => val,
                Err(err) => {
                    tracing::error!("Failed to check if order exists in db: {err:?}");
                    continue;
                }
            };
            if order_exists {
                continue;
            }

            let req_status = match market.get_status(event.request.id).await {
                Ok(val) => val,
                Err(err) => {
                    tracing::warn!("Failed to get request status: {err:?}");
                    continue;
                }
            };

            if !matches!(req_status, ProofStatus::Unknown) {
                tracing::debug!(
                    "Skipping order {} reason: order status no longer bidding: {:?}",
                    event.request.id,
                    req_status
                );
                continue;
            }

            tracing::info!("Found open order: {}", event.request.id);
            if let Err(err) = db
                .add_order(
                    event.request.id,
                    Order::new(event.request.clone(), event.clientSignature.clone()),
                )
                .await
            {
                tracing::error!("Failed to insert order in to database: {err:?}");
                continue;
            }
            order_count += 1;
        }

        tracing::info!("Found {order_count} open orders");

        Ok(order_count)
    }

    async fn monitor_orders(market_addr: Address, provider: Arc<P>, db: DbObj) -> Result<()> {
        let market = ProofMarketService::new(market_addr, provider, Address::ZERO);
        // TODO: RPC providers can drop filters over time or flush them
        // we should try and move this to a subscription filter if we have issue with the RPC
        // dropping filters

        let event = market.instance().RequestSubmitted_filter().watch().await?;
        tracing::info!("Subscribed to RequestSubmitted event");
        event
            .into_stream()
            .for_each(|log_res| async {
                match log_res {
                    Ok((event, _log)) => {
                        tracing::info!("Detected new request {:x}", event.request.id);
                        if let Err(err) = db
                            .add_order(
                                event.request.id,
                                Order::new(event.request, event.clientSignature),
                            )
                            .await
                        {
                            tracing::error!("Failed to add new order into DB: {err:?}");
                        }
                    }
                    Err(err) => {
                        tracing::warn!("Failed to fetch event log: {:?}", err);
                    }
                }
            })
            .await;

        Ok(())
    }
}

impl<T, P> RetryTask for MarketMonitor<T, P>
where
    T: Transport + Clone,
    P: Provider<T, Ethereum> + 'static + Clone,
{
    fn spawn(&self) -> RetryRes {
        let lookback_blocks = self.lookback_blocks;
        let market_addr = self.market_addr;
        let provider = self.provider.clone();
        let db = self.db.clone();

        Box::pin(async move {
            tracing::info!("Starting up market monitor");

            Self::find_open_orders(lookback_blocks, market_addr, provider.clone(), db.clone())
                .await
                .map_err(SupervisorErr::Fault)?;

            Self::monitor_orders(market_addr, provider, db).await.map_err(|err| {
                tracing::error!("Monitor for new blocks failed, restarting: {err:?}");

                SupervisorErr::Recover(err)
            })?;

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
        providers::{ext::AnvilApi, ProviderBuilder, WalletProvider},
        signers::local::PrivateKeySigner,
    };
    use boundless_market::contracts::{
        proof_market::ProofMarketService, test_utils::ProofMarket, Input, InputType, Offer,
        Predicate, PredicateType, ProvingRequest, Requirements,
    };

    #[tokio::test]
    async fn find_orders() {
        let anvil = Anvil::new().spawn();
        let signer: PrivateKeySigner = anvil.keys()[0].clone().into();
        let provider = ProviderBuilder::new()
            .with_recommended_fillers()
            .wallet(EthereumWallet::from(signer.clone()))
            .on_http(anvil.endpoint().parse().unwrap());
        let contract_address =
            *ProofMarket::deploy(&provider, Address::ZERO, B256::ZERO).await.unwrap().address();
        let proof_market = ProofMarketService::new(
            contract_address,
            provider.clone(),
            provider.default_signer_address(),
        );

        let min_price = 1;
        let max_price = 10;
        let proving_request = ProvingRequest {
            id: U256::ZERO,
            requirements: Requirements {
                imageId: B256::ZERO,
                predicate: Predicate {
                    predicateType: PredicateType::PrefixMatch,
                    data: Default::default(),
                },
            },
            imageUrl: "test".to_string(),
            input: Input { inputType: InputType::Url, data: Default::default() },
            offer: Offer {
                minPrice: min_price,
                maxPrice: max_price,
                biddingStart: 0,
                timeout: 1000,
                rampUpPeriod: 1,
                lockinStake: 0,
            },
        };

        proof_market.submit_request(&proving_request, &signer).await.unwrap();

        // let event: Event<_, _, IProofMarket::RequestSubmitted, _> = Event::new(&provider,
        // Filter::new());

        // tx_receipt.inner.logs().into_iter().map(|log| Ok((decode_log(&log)?, log))).collect()

        let db: DbObj = Arc::new(SqliteDb::new("sqlite::memory:").await.unwrap());
        let orders = MarketMonitor::find_open_orders(2, contract_address, Arc::new(provider), db)
            .await
            .unwrap();
        assert_eq!(orders, 1);
    }

    #[tokio::test]
    async fn block_times() {
        let anvil = Anvil::new().spawn();
        let signer: PrivateKeySigner = anvil.keys()[0].clone().into();
        let provider = ProviderBuilder::new()
            .with_recommended_fillers()
            .wallet(EthereumWallet::from(signer))
            .on_http(anvil.endpoint().parse().unwrap());

        provider.anvil_mine(Some(U256::from(10)), Some(U256::from(2))).await.unwrap();

        let db: DbObj = Arc::new(SqliteDb::new("sqlite::memory:").await.unwrap());
        let market_monitor = MarketMonitor::new(1, Address::ZERO, Arc::new(provider), db);

        let block_time = market_monitor.get_block_time().await.unwrap();
        assert_eq!(block_time, 2);
    }
}
