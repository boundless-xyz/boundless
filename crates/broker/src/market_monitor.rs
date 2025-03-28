// Copyright (c) 2025 RISC Zero, Inc.
//
// All rights reserved.

use std::sync::Arc;

use alloy::{
    consensus::Transaction,
    network::Ethereum,
    primitives::{Address, FixedBytes, U256},
    providers::Provider,
    rpc::types::{Filter, Log},
    sol,
    sol_types::{SolCall, SolEvent, SolStruct},
};

use anyhow::{Context, Result};
use boundless_market::contracts::{
    boundless_market::BoundlessMarketService,
    eip712_domain,
    IBoundlessMarket::{self, IBoundlessMarketInstance},
    ProofRequest, ProofStatus, RequestId,
};
use futures_util::StreamExt;

use crate::{
    chain_monitor::ChainMonitorService,
    db::DbError,
    task::{RetryRes, RetryTask, SupervisorErr},
    DbObj, Order,
};

const BLOCK_TIME_SAMPLE_SIZE: u64 = 10;

pub struct MarketMonitor<P> {
    lookback_blocks: u64,
    market_addr: Address,
    provider: Arc<P>,
    db: DbObj,
    chain_monitor: Arc<ChainMonitorService<P>>,
}

sol! {
    #[sol(rpc)]
    interface IERC1271 {
        function isValidSignature(bytes32 hash, bytes memory signature) external view returns (bytes4 magicValue);
    }
}

const ERC1271_MAGIC_VALUE: [u8; 4] = [0x16, 0x26, 0xba, 0x7e];

impl<P> MarketMonitor<P>
where
    P: Provider<Ethereum> + 'static + Clone,
{
    pub fn new(
        lookback_blocks: u64,
        market_addr: Address,
        provider: Arc<P>,
        db: DbObj,
        chain_monitor: Arc<ChainMonitorService<P>>,
    ) -> Self {
        Self { lookback_blocks, market_addr, provider, db, chain_monitor }
    }

    /// Queries chain history to sample for the median block time
    pub async fn get_block_time(&self) -> Result<u64> {
        let current_block = self.chain_monitor.current_block_number().await?;

        let mut timestamps = vec![];
        let sample_start = current_block - std::cmp::min(current_block, BLOCK_TIME_SAMPLE_SIZE);
        for i in sample_start..current_block {
            let block = self
                .provider
                .get_block_by_number(i.into(), false.into())
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
        chain_monitor: Arc<ChainMonitorService<P>>,
    ) -> Result<u64> {
        let current_block = chain_monitor.current_block_number().await?;

        let start_block = current_block.saturating_sub(lookback_blocks);

        tracing::info!("Searching for existing open orders: {start_block} - {current_block}");

        let market = BoundlessMarketService::new(market_addr, provider.clone(), Address::ZERO);
        // let event: Event<_, _, IBoundlessMarket::RequestSubmitted, _> = Event::new(
        //     provider.clone(),
        //     Filter::new().from_block(start_block).address(market_addr),
        // );

        // let logs = event.query().await.context("Failed to query RequestSubmitted events")?;

        let filter = Filter::new()
            .event_signature(IBoundlessMarket::RequestSubmitted::SIGNATURE_HASH)
            .from_block(start_block)
            .address(market_addr);

        // TODO: This could probably be cleaned up but the alloy examples
        // don't have a lot of clean log decoding samples, and the Event::query()
        // interface would randomly fail for me?
        let logs = provider.get_logs(&filter).await?;
        let decoded_logs = logs.iter().filter_map(|log| {
            match log.log_decode::<IBoundlessMarket::RequestSubmitted>() {
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
            let request_id = U256::from(event.requestId);
            let tx_hash = log.transaction_hash.context("Missing transaction hash")?;
            let tx_data = provider
                .get_transaction_by_hash(tx_hash)
                .await?
                .context("Missing transaction data")?;
            let calldata = IBoundlessMarket::submitRequestCall::abi_decode(tx_data.input(), true)
                .context("Failed to decode calldata")?;
            let order_exists = match db.order_exists(request_id).await {
                Ok(val) => val,
                Err(err) => {
                    tracing::error!("Failed to check if order exists in db: {err:?}");
                    continue;
                }
            };
            // TODO(#162) Handle the case where multiple requests share an ID.
            if order_exists {
                continue;
            }

            let req_status =
                match market.get_status(request_id, Some(calldata.request.expires_at())).await {
                    Ok(val) => val,
                    Err(err) => {
                        tracing::warn!("Failed to get request status: {err:?}");
                        continue;
                    }
                };

            if !matches!(req_status, ProofStatus::Unknown) {
                tracing::debug!(
                    "Skipping order {} reason: order status no longer bidding: {:?}",
                    calldata.request.id,
                    req_status
                );
                continue;
            }

            tracing::info!("Found open order: {}", calldata.request.id);
            if let Err(err) = db
                .add_order(
                    request_id,
                    Order::new(calldata.request.clone(), calldata.clientSignature.clone()),
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
        let chain_id = provider.get_chain_id().await?;

        let market = BoundlessMarketService::new(market_addr, provider.clone(), Address::ZERO);
        // TODO: RPC providers can drop filters over time or flush them
        // we should try and move this to a subscription filter if we have issue with the RPC
        // dropping filters

        let event = market.instance().RequestSubmitted_filter().watch().await?;
        tracing::info!("Subscribed to RequestSubmitted event");
        event
            .into_stream()
            .for_each(|log_res| async {
                match log_res {
                    Ok((event, log)) => {
                        if let Err(err) = Self::process_log(
                            event,
                            log,
                            provider.clone(),
                            market_addr,
                            chain_id,
                            &db,
                        )
                        .await
                        {
                            tracing::error!("Failed to process event log: {err:?}");
                        }
                    }
                    Err(err) => {
                        tracing::warn!("Failed to fetch event log: {:?}", err);
                    }
                }
            })
            .await;

        anyhow::bail!("Event polling exited, polling failed (possible RPC error)");
    }

    async fn process_log(
        event: IBoundlessMarket::RequestSubmitted,
        log: Log,
        provider: Arc<P>,
        market_addr: Address,
        chain_id: u64,
        db: &DbObj,
    ) -> Result<()> {
        tracing::info!("Detected new request {:x}", event.requestId);

        let tx_hash = log.transaction_hash.context("Missing transaction hash")?;
        let tx_data =
            provider.get_transaction_by_hash(tx_hash).await?.context("Missing transaction data")?;

        let calldata = IBoundlessMarket::submitRequestCall::abi_decode(tx_data.input(), true)
            .context("Failed to decode calldata")?;

        // Check the request id flag to determine if the request is smart contract signed. If so we validate the
        // ERC1271 signature by calling isValidSignature on the smart contract client. Otherwise we validate the
        // signature locally.
        let request_id = RequestId::from_lossy(calldata.request.id);
        if request_id.smart_contract_signed {
            let erc1271 = IERC1271::new(request_id.addr, provider);
            let request_hash = calldata.request.signing_hash(market_addr, chain_id)?;
            tracing::info!(
                "Validating ERC1271 signature for request 0x{:x}, calling contract: {} with hash {:x}",
                calldata.request.id,
                request_id.addr,
                request_hash
            );
            match erc1271
                .isValidSignature(request_hash, calldata.clientSignature.clone())
                .call()
                .await
            {
                Ok(res) => {
                    let magic_value = res.magicValue;
                    if magic_value != ERC1271_MAGIC_VALUE {
                        tracing::warn!("Invalid ERC1271 signature for request 0x{:x}, contract: {} returned magic value: 0x{:x}", calldata.request.id, request_id.addr, magic_value);
                        return Ok(());
                    }
                }
                Err(err) => {
                    tracing::warn!("Failed to call ERC1271 isValidSignature for request 0x{:x}, contract: {} - {err:?}", calldata.request.id, request_id.addr);
                    return Ok(());
                }
            }
        } else if let Err(err) =
            calldata.request.verify_signature(&calldata.clientSignature, market_addr, chain_id)
        {
            tracing::warn!(
                "Failed to validate order signature: 0x{:x} - {err:?}",
                calldata.request.id
            );
            return Ok(()); // Return early without propagating the error if signature verification fails.
        }

        if let Err(err) = db
            .add_order(
                U256::from(calldata.request.id),
                Order::new(calldata.request, calldata.clientSignature),
            )
            .await
        {
            match err {
                DbError::SqlErr(sqlx::Error::Database(db_err)) => {
                    if db_err.is_unique_violation() {
                        tracing::warn!("Duplicate order detected: {db_err:?}");
                    } else {
                        tracing::error!("Failed to add new order into DB: {db_err:?}");
                    }
                }
                _ => {
                    tracing::error!("Failed to add new order into DB: {err:?}");
                }
            }
        }
        Ok(())
    }
}

impl<P> RetryTask for MarketMonitor<P>
where
    P: Provider<Ethereum> + 'static + Clone,
{
    fn spawn(&self) -> RetryRes {
        let lookback_blocks = self.lookback_blocks;
        let market_addr = self.market_addr;
        let provider = self.provider.clone();
        let db = self.db.clone();
        let chain_monitor = self.chain_monitor.clone();

        Box::pin(async move {
            tracing::info!("Starting up market monitor");

            Self::find_open_orders(
                lookback_blocks,
                market_addr,
                provider.clone(),
                db.clone(),
                chain_monitor,
            )
            .await
            .map_err(|err| {
                tracing::error!("Monitor failed to find open orders on startup: {err:?}");
                SupervisorErr::Recover(err)
            })?;

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
    use crate::{db::SqliteDb, now_timestamp};
    use alloy::{
        network::EthereumWallet,
        node_bindings::Anvil,
        primitives::{Address, U256},
        providers::{ext::AnvilApi, ProviderBuilder, WalletProvider},
        signers::local::PrivateKeySigner,
    };
    use boundless_market::contracts::{
        boundless_market::BoundlessMarketService, test_utils::deploy_boundless_market, Input,
        InputType, Offer, Predicate, PredicateType, ProofRequest, Requirements,
    };
    use guest_assessor::ASSESSOR_GUEST_ID;
    use risc0_zkvm::sha::Digest;

    #[tokio::test]
    async fn find_orders() {
        let anvil = Anvil::new().spawn();
        let signer: PrivateKeySigner = anvil.keys()[0].clone().into();
        let provider = Arc::new(
            ProviderBuilder::new()
                .wallet(EthereumWallet::from(signer.clone()))
                .on_builtin(&anvil.endpoint())
                .await
                .unwrap(),
        );

        let market_address = deploy_boundless_market(
            signer.address(),
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

        let min_price = 1;
        let max_price = 10;
        let proving_request = ProofRequest {
            id: boundless_market.request_id_from_nonce().await.unwrap(),
            requirements: Requirements::new(
                Digest::ZERO,
                Predicate { predicateType: PredicateType::PrefixMatch, data: Default::default() },
            ),
            imageUrl: "test".to_string(),
            input: Input { inputType: InputType::Url, data: Default::default() },
            offer: Offer {
                minPrice: U256::from(min_price),
                maxPrice: U256::from(max_price),
                biddingStart: now_timestamp() - 5,
                timeout: 1000,
                lockTimeout: 1000,
                rampUpPeriod: 1,
                lockStake: U256::from(0),
            },
        };

        boundless_market.submit_request(&proving_request, &signer).await.unwrap();

        // let event: Event<_, _, IBoundlessMarket::RequestSubmitted, _> = Event::new(&provider,
        // Filter::new());

        // tx_receipt.inner.logs().into_iter().map(|log| Ok((decode_log(&log)?, log))).collect()

        let chain_monitor = Arc::new(ChainMonitorService::new(provider.clone()).await.unwrap());
        tokio::spawn(chain_monitor.spawn());

        let db: DbObj = Arc::new(SqliteDb::new("sqlite::memory:").await.unwrap());
        let orders =
            MarketMonitor::find_open_orders(2, market_address, provider, db, chain_monitor)
                .await
                .unwrap();
        assert_eq!(orders, 1);
    }

    #[tokio::test]
    async fn block_times() {
        let anvil = Anvil::new().spawn();
        let signer: PrivateKeySigner = anvil.keys()[0].clone().into();
        let provider = Arc::new(
            ProviderBuilder::new()
                .wallet(EthereumWallet::from(signer))
                .on_builtin(&anvil.endpoint())
                .await
                .unwrap(),
        );

        provider.anvil_mine(Some(10), Some(2)).await.unwrap();

        let chain_monitor = Arc::new(ChainMonitorService::new(provider.clone()).await.unwrap());
        tokio::spawn(chain_monitor.spawn());
        let db: DbObj = Arc::new(SqliteDb::new("sqlite::memory:").await.unwrap());
        let market_monitor = MarketMonitor::new(1, Address::ZERO, provider, db, chain_monitor);

        let block_time = market_monitor.get_block_time().await.unwrap();
        assert_eq!(block_time, 2);
    }
}
