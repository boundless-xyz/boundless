// Copyright (c) 2024 RISC Zero, Inc.
//
// All rights reserved.

use alloy::primitives::U256;
use anyhow::Result;
use futures_util::StreamExt;
use order_stream::{client::Client as OrderStreamClient, order_stream};

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

    async fn monitor_orders(client: OrderStreamClient, db: DbObj) -> Result<()> {
        let socket = client.connect_async().await?;
        let stream = order_stream(socket);
        tracing::info!("Subscribed to offchain Order stream");
        stream
            .for_each(|order| async {
                match order {
                    Ok(order) => {
                        tracing::info!("Detected new order {:x}", order.request.id);
                        if let Err(err) = db
                            .add_order(
                                U256::from(order.request.id),
                                Order::new(order.request, order.signature.as_bytes().into()),
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

        anyhow::bail!("offchain Order stream polling exited, polling failed");
    }
}

impl RetryTask for OffchainMarketMonitor {
    fn spawn(&self) -> RetryRes {
        let db = self.db.clone();
        let client = self.client.clone();

        Box::pin(async move {
            tracing::info!("Starting up offchain market monitor");

            Self::monitor_orders(client, db).await.map_err(|err| {
                tracing::error!("Monitor for offchain orders failed, restarting: {err:?}");

                SupervisorErr::Recover(err)
            })?;

            Ok(())
        })
    }
}
