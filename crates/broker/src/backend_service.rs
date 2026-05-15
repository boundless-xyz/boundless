// Copyright 2026 Boundless Foundation, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! Broker-facing backend service boundary.
//!
//! This module starts as a thin adapter over the existing RISC Zero broker code.
//! The broker keeps DB state, retry policy, cancellation, and batch lifecycle
//! orchestration; the backend service owns zkVM-specific proof processing.

use async_trait::async_trait;

use crate::{
    provers::{self, ProverObj},
    requestor_monitor::PriorityRequestors,
    storage::{upload_image_uri, upload_input_uri},
    CompressionType, ConfigurableDownloader, Order, OrderStatus,
};
use anyhow::{Context, Result};

/// Command for processing one accepted broker order.
#[derive(Clone, Debug)]
pub struct ProcessOrder {
    pub order: Order,
}

/// Durable progress returned while an order is being processed.
#[derive(Clone, Debug, PartialEq)]
pub enum OrderProcessProgress {
    Started { proof_id: String },
    Compressed { proof_id: String, compressed_proof_id: String },
    Completed(ProcessedOrder),
}

/// Completed backend processing for one broker order.
#[derive(Clone, Debug, PartialEq)]
pub struct ProcessedOrder {
    pub order_id: String,
    pub proof_id: String,
    pub compressed_proof_id: Option<String>,
    pub next_status: OrderStatus,
}

#[async_trait]
pub trait BackendService: Send + Sync {
    async fn process_order(&self, cmd: ProcessOrder) -> Result<OrderProcessProgress>;
}

#[derive(Clone)]
pub struct Risc0BackendService {
    prover: ProverObj,
    snark_prover: ProverObj,
    downloader: ConfigurableDownloader,
    priority_requestors: PriorityRequestors,
}

impl Risc0BackendService {
    pub fn new(
        prover: ProverObj,
        snark_prover: ProverObj,
        downloader: ConfigurableDownloader,
        priority_requestors: PriorityRequestors,
    ) -> Self {
        Self { prover, snark_prover, downloader, priority_requestors }
    }

    async fn start_order(&self, order: &Order) -> Result<String> {
        let order_id = order.id();

        if let Some(existing_proof_id) = order.proof_id.clone() {
            tracing::debug!("Using existing proof {existing_proof_id} for order {order_id}");
            return Ok(existing_proof_id);
        }

        tracing::info!("Proving order {order_id}");

        let image_id = match order.image_id.as_ref() {
            Some(val) => val.clone(),
            None => upload_image_uri(&self.prover, &order.request, &self.downloader)
                .await
                .with_context(|| format!("Failed to upload image for order {order_id}"))?,
        };

        let input_id = match order.input_id.as_ref() {
            Some(val) => val.clone(),
            None => upload_input_uri(
                &self.prover,
                &order.request,
                &self.downloader,
                &self.priority_requestors,
            )
            .await
            .with_context(|| format!("Failed to upload input for order {order_id}"))?,
        };

        let proof_id = self
            .prover
            .prove_stark(&image_id, &input_id, /* TODO assumptions */ vec![])
            .await
            .with_context(|| format!("Failed to prove customer proof STARK order {order_id}"))?;

        tracing::debug!("Order {order_id} being proved, proof id: {proof_id}");
        Ok(proof_id)
    }

    async fn compress_order_proof(
        &self,
        order_id: &str,
        stark_proof_id: &str,
        compression_type: CompressionType,
    ) -> Result<String> {
        let proof_id = match compression_type {
            CompressionType::Groth16 => self
                .snark_prover
                .compress(stark_proof_id)
                .await
                .with_context(|| format!("Failed to compress proof for order {order_id}"))?,
            CompressionType::Blake3Groth16 => {
                self.snark_prover.compress_blake3_groth16(stark_proof_id).await.with_context(
                    || format!("Failed to compress blake3 groth16 proof for order {order_id}"),
                )?
            }
            CompressionType::None => unreachable!("compression type should not be None"),
        };

        match compression_type {
            CompressionType::Groth16 => {
                tracing::trace!(
                    "Verifying compressed Groth16 receipt locally for proof_id: {proof_id}, order {order_id}"
                );
                provers::verify_groth16_receipt(&self.snark_prover, &proof_id).await?;
            }
            CompressionType::Blake3Groth16 => {
                tracing::trace!(
                    "Verifying compressed Blake3 Groth16 receipt locally for proof_id: {proof_id}, order {order_id}"
                );
                provers::verify_blake3_groth16_receipt(&self.snark_prover, &proof_id).await?;
            }
            CompressionType::None => unreachable!("compression type should not be None"),
        }

        Ok(proof_id)
    }
}

#[async_trait]
impl BackendService for Risc0BackendService {
    async fn process_order(&self, cmd: ProcessOrder) -> Result<OrderProcessProgress> {
        let order = cmd.order;
        let order_id = order.id();

        let Some(stark_proof_id) = order.proof_id.clone() else {
            return Ok(OrderProcessProgress::Started { proof_id: self.start_order(&order).await? });
        };

        let proof_res = self
            .prover
            .wait_for_stark(&stark_proof_id)
            .await
            .context("Monitoring proof (stark) failed")?;

        let compression_type = order.compression_type();
        tracing::debug!(
            "Order {order_id} has compression_type: {compression_type:?}, snark_proof_id: {:?}",
            order.compressed_proof_id
        );

        if compression_type != CompressionType::None && order.compressed_proof_id.is_none() {
            let compressed_proof_id =
                self.compress_order_proof(&order_id, &stark_proof_id, compression_type).await?;
            return Ok(OrderProcessProgress::Compressed {
                proof_id: stark_proof_id,
                compressed_proof_id,
            });
        }

        let next_status = match compression_type {
            CompressionType::None => OrderStatus::PendingAgg,
            CompressionType::Groth16 | CompressionType::Blake3Groth16 => {
                OrderStatus::SkipAggregation
            }
        };

        tracing::info!(
            "Customer Proof complete for proof_id: {stark_proof_id}, order_id: {order_id} cycles: {:?} time: {}",
            proof_res.stats.as_ref().map(|s| s.total_cycles),
            proof_res.elapsed_time,
        );

        Ok(OrderProcessProgress::Completed(ProcessedOrder {
            order_id,
            proof_id: stark_proof_id,
            compressed_proof_id: order.compressed_proof_id,
            next_status,
        }))
    }
}
