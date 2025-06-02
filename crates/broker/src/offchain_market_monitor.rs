// Copyright (c) 2025 RISC Zero, Inc.
//
// All rights reserved.

use alloy::signers::{local::PrivateKeySigner, Signer};
use anyhow::Result;
use boundless_market::order_stream_client::{order_stream, OrderStreamClient};
use futures::TryStreamExt;
use futures_util::StreamExt;

use crate::{
    errors::CodedError,
    impl_coded_debug,
    task::{RetryRes, RetryTask, SupervisorErr},
    FulfillmentType, OrderRequest,
};
use thiserror::Error;

#[derive(Error)]
pub enum OffchainMarketMonitorErr {
    #[error("WebSocket error: {0:?}")]
    WebSocketErr(anyhow::Error),

    #[error("{code} Receiver dropped", code = self.code())]
    ReceiverDropped,

    #[error("{code} Unexpected error: {0:?}", code = self.code())]
    UnexpectedErr(#[from] anyhow::Error),
}

impl_coded_debug!(OffchainMarketMonitorErr);

impl CodedError for OffchainMarketMonitorErr {
    fn code(&self) -> &str {
        match self {
            OffchainMarketMonitorErr::WebSocketErr(_) => "[B-OMM-001]",
            OffchainMarketMonitorErr::ReceiverDropped => "[B-OMM-002]",
            OffchainMarketMonitorErr::UnexpectedErr(_) => "[B-OMM-500]",
        }
    }
}

pub struct OffchainMarketMonitor {
    client: OrderStreamClient,
    signer: PrivateKeySigner,
    new_order_tx: tokio::sync::mpsc::Sender<Box<OrderRequest>>,
}

impl OffchainMarketMonitor {
    pub fn new(
        client: OrderStreamClient,
        signer: PrivateKeySigner,
        new_order_tx: tokio::sync::mpsc::Sender<Box<OrderRequest>>,
    ) -> Self {
        Self { client, signer, new_order_tx }
    }

    async fn monitor_orders(
        client: OrderStreamClient,
        signer: &impl Signer,
        new_order_tx: tokio::sync::mpsc::Sender<Box<OrderRequest>>,
    ) -> Result<(), OffchainMarketMonitorErr> {
        tracing::debug!("Connecting to off-chain market: {}", client.base_url);
        let socket =
            client.connect_async(signer).await.map_err(OffchainMarketMonitorErr::WebSocketErr)?;

        let stream = order_stream(socket);
        tracing::info!("Subscribed to offchain Order stream");
        stream
            .map(Ok)
            .try_for_each_concurrent(Some(10), |order_data| {
                let new_order_tx = new_order_tx.clone();
                async move {
                    tracing::info!(
                        "Detected new order with stream id {:x}, request id: {:x}",
                        order_data.id,
                        order_data.order.request.id
                    );

                    let new_order = OrderRequest::new(
                        order_data.order.request,
                        order_data.order.signature.as_bytes().into(),
                        FulfillmentType::LockAndFulfill,
                        client.boundless_market_address,
                        client.chain_id,
                    );

                    if let Err(e) = new_order_tx.send(Box::new(new_order)).await {
                        tracing::error!("Failed to send new order to broker: {}", e);
                        Err(OffchainMarketMonitorErr::ReceiverDropped)
                    } else {
                        tracing::trace!(
                            "Sent new off-chain order {:x} to OrderPicker via channel.",
                            order_data.id
                        );
                        Ok(())
                    }
                }
            })
            .await?;

        Err(OffchainMarketMonitorErr::WebSocketErr(anyhow::anyhow!(
            "Offchain order stream websocket exited, polling failed"
        )))
    }
}

impl RetryTask for OffchainMarketMonitor {
    type Error = OffchainMarketMonitorErr;
    fn spawn(&self) -> RetryRes<Self::Error> {
        let client = self.client.clone();
        let signer = self.signer.clone();
        let new_order_tx = self.new_order_tx.clone();

        Box::pin(async move {
            tracing::info!("Starting up offchain market monitor");
            Self::monitor_orders(client, &signer, new_order_tx)
                .await
                .map_err(SupervisorErr::Recover)?;
            Ok(())
        })
    }
}
