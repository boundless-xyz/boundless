// Copyright (c) 2025 RISC Zero, Inc.
//
// All rights reserved.

use alloy::{
    primitives::U256,
    signers::{local::PrivateKeySigner, Signer},
};
use anyhow::Result;
use boundless_market::order_stream_client::{order_stream, Client as OrderStreamClient};
use futures_util::StreamExt;

use crate::{
    task::{RetryRes, RetryTask, SupervisorErr},
    DbObj, FulfillmentType, Order,
};

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
    ) -> Result<(), SupervisorErr> {
        // TODO: retry until it can reestablish a connection.
        tracing::debug!("Connecting to off-chain market: {}", client.base_url);
        let socket = client.connect_async(signer).await.map_err(SupervisorErr::Recover)?;

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
                                FulfillmentType::FulfillAfterLockExpire,
                                client.boundless_market_address,
                                client.chain_id,
                            ))
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
        let signer = self.signer.clone();

        Box::pin(async move {
            tracing::info!("Starting up offchain market monitor");
            Self::monitor_orders(client, &signer, db).await?;
            Ok(())
        })
    }
}
