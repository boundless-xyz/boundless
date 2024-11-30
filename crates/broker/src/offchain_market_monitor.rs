// Copyright (c) 2024 RISC Zero, Inc.
//
// All rights reserved.

use alloy::primitives::U256;
use anyhow::Result;
use boundless_market::order_stream_client::{order_stream, Client as OrderStreamClient};
use futures_util::StreamExt;

use crate::{
    task::{RetryRes, RetryTask, SupervisorErr},
    DbObj, Order,
};

pub struct OffchainMarketMonitor {
    db: DbObj,
    client: OrderStreamClient,
}

impl OffchainMarketMonitor {
    pub fn new(db: DbObj, client: OrderStreamClient) -> Self {
        Self { db, client }
    }

    async fn monitor_orders(client: OrderStreamClient, db: DbObj) -> Result<(), SupervisorErr> {
        // TODO: retry until it can reestablish a connection.
        tracing::debug!("Connecting to off-chain market: {}", client.base_url);
        let socket = client.connect_async().await.map_err(SupervisorErr::Fault)?;

        let stream = order_stream(socket);
        tracing::info!("Subscribed to offchain Order stream");
        stream
            .for_each(|order| async {
                match order {
                    Ok(elm) => {
                        tracing::info!(
                            "Detected new order {:x} - stream id: {}",
                            elm.id,
                            elm.order.request.id
                        );
                        if let Err(err) = db
                            .add_order(
                                U256::from(elm.order.request.id),
                                Order::new(
                                    elm.order.request,
                                    elm.order.signature.as_bytes().into(),
                                ),
                            )
                            .await
                        {
                            tracing::error!("Failed to add new order into DB: {err:?}");
                        }
                    }
                    Err(err) => {
                        tracing::warn!("Failed to fetch order: {:?}", err);
                    }
                }
            })
            .await;

        Err(SupervisorErr::Recover(anyhow::anyhow!(
            "offchain Order stream polling exited, polling failed"
        )))
    }
}

impl RetryTask for OffchainMarketMonitor {
    fn spawn(&self) -> RetryRes {
        let db = self.db.clone();
        let client = self.client.clone();

        Box::pin(async move {
            tracing::info!("Starting up offchain market monitor");
            Self::monitor_orders(client, db).await?;
            Ok(())
        })
    }
}
