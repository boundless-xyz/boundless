// Copyright (c) 2025 RISC Zero, Inc.
//
// All rights reserved.

use alloy::signers::{local::PrivateKeySigner, Signer};
use anyhow::{Context, Result};
use boundless_market::order_stream_client::{order_stream, OrderStreamClient as OrderStreamClient};
use futures_util::StreamExt;

use crate::{
    errors::CodedError,
    impl_coded_debug,
    task::{RetryRes, RetryTask, SupervisorErr},
    DbObj, FulfillmentType, Order,
};
use thiserror::Error;

#[derive(Error)]
pub enum OffchainMarketMonitorErr {
    #[error("WebSocket error: {0:?}")]
    WebSocketErr(anyhow::Error),

    #[error("{code} Unexpected error: {0:?}", code = self.code())]
    UnexpectedErr(#[from] anyhow::Error),
}

impl_coded_debug!(OffchainMarketMonitorErr);

impl CodedError for OffchainMarketMonitorErr {
    fn code(&self) -> &str {
        match self {
            OffchainMarketMonitorErr::WebSocketErr(_) => "[B-OMM-001]",
            OffchainMarketMonitorErr::UnexpectedErr(_) => "[B-OMM-500]",
        }
    }
}

pub struct OffchainMarketMonitor {
    db: DbObj,
    client: OrderStreamClient,
    signer: PrivateKeySigner,
}

impl OffchainMarketMonitor {
    pub fn new(db: DbObj, client: OrderStreamClient, signer: PrivateKeySigner) -> Self {
        Self { db, client, signer }
    }

    async fn monitor_orders(
        client: OrderStreamClient,
        signer: &impl Signer,
        db: DbObj,
    ) -> Result<(), OffchainMarketMonitorErr> {
        tracing::debug!("Connecting to off-chain market: {}", client.base_url);
        let socket =
            client.connect_async(signer).await.context("Failed to connect to offchain market")?;

        let stream = order_stream(socket);
        tracing::info!("Subscribed to offchain Order stream");
        stream
            .for_each(|order| async {
                match order {
                    Ok(elm) => {
                        tracing::info!(
                            "Detected new order with stream id {:x}, request id: {:x}",
                            elm.id,
                            elm.order.request.id
                        );
                        if let Err(err) = db
                            .add_order(Order::new(
                                elm.order.request,
                                elm.order.signature.as_bytes().into(),
                                FulfillmentType::LockAndFulfill,
                                client.boundless_market_address,
                                client.chain_id,
                            ))
                            .await
                        {
                            tracing::error!(
                                "{} Failed to add new order into DB: {err:?}",
                                err.code()
                            );
                        }
                    }
                    Err(err) => {
                        tracing::warn!("Failed to fetch order: {:?}", err);
                    }
                }
            })
            .await;

        Err(OffchainMarketMonitorErr::WebSocketErr(anyhow::anyhow!(
            "Offchain order stream websocket exited, polling failed"
        )))
    }
}

impl RetryTask for OffchainMarketMonitor {
    type Error = OffchainMarketMonitorErr;
    fn spawn(&self) -> RetryRes<Self::Error> {
        let db = self.db.clone();
        let client = self.client.clone();
        let signer = self.signer.clone();

        Box::pin(async move {
            tracing::info!("Starting up offchain market monitor");
            Self::monitor_orders(client, &signer, db).await.map_err(SupervisorErr::Recover)?;
            Ok(())
        })
    }
}
