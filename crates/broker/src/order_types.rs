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

//! Order types and helpers shared across broker services.
//!
//! Defines the persistent [`Order`] domain object and its lifecycle status
//! ([`OrderStatus`]), the on-chain state change events ([`OrderStateChange`])
//! broadcast from chain monitors, and the compression-type discriminator
//! ([`CompressionType`]) derived from a request's selector. Plus the small
//! helpers that build an [`Order`] from a fresh [`OrderRequest`].

use std::sync::OnceLock;

use alloy::primitives::{Address, Bytes, FixedBytes, U256};
use boundless_market::{
    contracts::ProofRequest,
    prover_utils::{FulfillmentType, OrderRequest},
    selector::{is_blake3_groth16_selector, is_groth16_selector},
};
use chrono::{serde::ts_seconds, DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Status of a persistent order as it moves through the lifecycle in the database.
/// Orders in initial, intermediate, or terminal non-failure states (e.g. New, Pricing, Done, Skipped)
/// are managed in-memory or removed from the database.
#[derive(Clone, Copy, sqlx::Type, Debug, PartialEq, Serialize, Deserialize)]
pub enum OrderStatus {
    /// Order is ready to commence proving (either locked or filling without locking)
    PendingProving,
    /// Order is actively ready for proving
    Proving,
    /// Order is ready for aggregation
    PendingAgg,
    /// Order is in the process of Aggregation
    Aggregating,
    /// Unaggregated order is ready for submission
    SkipAggregation,
    /// Pending on chain finalization
    PendingSubmission,
    /// Order has been completed
    Done,
    /// Order failed
    Failed,
    /// Order was analyzed and marked as skipable
    Skipped,
}

/// On-chain order state change broadcast from MarketMonitor to all unified components
/// (OrderEvaluator, OrderPricer, OrderCommitter) and the ProvingService.
///
/// Each variant carries `chain_id` to ensure that events on one chain don't affect
/// pending orders for other chains that may share the same `request_id`.
#[derive(Debug, Clone)]
pub enum OrderStateChange {
    Locked { request_id: U256, prover: Address, chain_id: u64 },
    Fulfilled { request_id: U256, chain_id: u64 },
}

impl OrderStateChange {
    /// Returns the chain that produced this state change event.
    pub fn chain_id(&self) -> u64 {
        match self {
            OrderStateChange::Locked { chain_id, .. } => *chain_id,
            OrderStateChange::Fulfilled { chain_id, .. } => *chain_id,
        }
    }
}

/// Helper function to format an order ID consistently
fn format_order_id(
    request_id: &U256,
    signing_hash: &FixedBytes<32>,
    fulfillment_type: &FulfillmentType,
) -> String {
    format!("0x{request_id:x}-{signing_hash}-{fulfillment_type:?}")
}

pub(crate) fn order_from_request(order_request: &OrderRequest, status: OrderStatus) -> Order {
    Order {
        boundless_market_address: order_request.boundless_market_address,
        chain_id: order_request.chain_id,
        fulfillment_type: order_request.fulfillment_type,
        request: order_request.request.clone(),
        status,
        client_sig: order_request.client_sig.clone(),
        updated_at: Utc::now(),
        image_id: order_request.image_id.clone(),
        input_id: order_request.input_id.clone(),
        total_cycles: order_request.total_cycles,
        journal_bytes: order_request.journal_bytes,
        target_timestamp: order_request.target_timestamp,
        expire_timestamp: order_request.expire_timestamp,
        proving_started_at: None,
        proof_id: None,
        compressed_proof_id: None,
        lock_price: None,
        error_msg: None,
        cached_id: OnceLock::new(),
    }
}

pub(crate) fn proving_order_from_request(order_request: &OrderRequest, lock_price: U256) -> Order {
    let mut order = order_from_request(order_request, OrderStatus::PendingProving);
    order.lock_price = Some(lock_price);
    order.proving_started_at = Some(Utc::now().timestamp().try_into().unwrap());
    order
}

/// An Order represents a proof request and a specific method of fulfillment.
///
/// Requests can be fulfilled in multiple ways, for example by locking then fulfilling them,
/// by waiting for an existing lock to expire then fulfilling for slashed stake, or by fulfilling
/// without locking at all.
///
/// For a given request, each type of fulfillment results in a separate Order being created, with different
/// FulfillmentType values.
///
/// Additionally, there may be multiple requests with the same request_id, but different ProofRequest
/// details. Those also result in separate Order objects being created.
///
/// See the id() method for more details on how Orders are identified.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Order {
    /// Address of the boundless market contract. Stored as it is required to compute the order id.
    pub(crate) boundless_market_address: Address,
    /// Chain ID of the boundless market contract. Stored as it is required to compute the order id.
    pub(crate) chain_id: u64,
    /// Fulfillment type
    pub(crate) fulfillment_type: FulfillmentType,
    /// Proof request object
    pub(crate) request: ProofRequest,
    /// status of the order
    pub(crate) status: OrderStatus,
    /// Last update time
    #[serde(with = "ts_seconds")]
    pub(crate) updated_at: DateTime<Utc>,
    /// Total cycles
    /// Populated after initial pricing in order picker
    pub(crate) total_cycles: Option<u64>,
    /// Journal size in bytes. Populated after preflight.
    #[serde(default)]
    pub(crate) journal_bytes: Option<usize>,
    /// Locking status target UNIX timestamp
    pub(crate) target_timestamp: Option<u64>,
    /// When proving was commenced at
    pub(crate) proving_started_at: Option<u64>,
    /// Prover image Id
    ///
    /// Populated after preflight
    pub(crate) image_id: Option<String>,
    /// Input Id
    ///
    ///  Populated after preflight
    pub(crate) input_id: Option<String>,
    /// Proof Id
    ///
    /// Populated after proof completion
    pub(crate) proof_id: Option<String>,
    /// Compressed proof Id
    ///
    /// Populated after proof completion. if the proof is compressed
    pub(crate) compressed_proof_id: Option<String>,
    /// UNIX timestamp the order expires at
    ///
    /// Populated during order picking
    pub(crate) expire_timestamp: Option<u64>,
    /// Client Signature
    pub(crate) client_sig: Bytes,
    /// Price the lockin was set at
    pub(crate) lock_price: Option<U256>,
    /// Failure message
    pub(crate) error_msg: Option<String>,
    #[serde(skip)]
    pub(crate) cached_id: OnceLock<String>,
}

impl Order {
    // An Order is identified by the request_id, the fulfillment type, and the hash of the proof request.
    // This structure supports multiple different ProofRequests with the same request_id, and different
    // fulfillment types.
    pub fn id(&self) -> String {
        self.cached_id
            .get_or_init(|| {
                let signing_hash = self
                    .request
                    .signing_hash(self.boundless_market_address, self.chain_id)
                    .unwrap();
                format_order_id(&self.request.id, &signing_hash, &self.fulfillment_type)
            })
            .clone()
    }

    pub fn is_groth16(&self) -> bool {
        is_groth16_selector(self.request.requirements.selector)
    }
    fn is_blake3_groth16(&self) -> bool {
        is_blake3_groth16_selector(self.request.requirements.selector)
    }
    pub fn compression_type(&self) -> CompressionType {
        if self.is_groth16() {
            CompressionType::Groth16
        } else if self.is_blake3_groth16() {
            CompressionType::Blake3Groth16
        } else {
            CompressionType::None
        }
    }
}

impl std::fmt::Display for Order {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let total_mcycles = if self.total_cycles.is_some() {
            format!(" ({} mcycles)", self.total_cycles.unwrap() / 1_000_000)
        } else {
            "".to_string()
        };
        write!(f, "{}{} [{}]", self.id(), total_mcycles, crate::format_expiries(&self.request))
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum CompressionType {
    None,
    Groth16,
    Blake3Groth16,
}

#[cfg(test)]
mod tests {
    use super::*;
    use boundless_market::contracts::{Offer, Predicate, RequestId, RequestInput, Requirements};
    use risc0_zkvm::sha::Digest;

    /// Ensures existing DB rows (serialized without journal_bytes) deserialize correctly.
    #[test]
    fn journal_bytes_backwards_compat() {
        let request = boundless_market::contracts::ProofRequest::new(
            RequestId::new(Address::ZERO, 0),
            Requirements::new(Predicate::prefix_match(Digest::ZERO, Bytes::default())),
            "",
            RequestInput::inline(Bytes::new()),
            Offer::default(),
        );

        // Create an Order and serialize it to JSON.
        let order = Order {
            boundless_market_address: Address::ZERO,
            chain_id: 1,
            fulfillment_type: FulfillmentType::LockAndFulfill,
            request,
            status: OrderStatus::PendingProving,
            updated_at: Utc::now(),
            total_cycles: Some(1000),
            journal_bytes: Some(42),
            target_timestamp: None,
            proving_started_at: None,
            image_id: None,
            input_id: None,
            proof_id: None,
            compressed_proof_id: None,
            expire_timestamp: None,
            client_sig: Bytes::new(),
            lock_price: None,
            error_msg: None,
            cached_id: OnceLock::new(),
        };
        let json = serde_json::to_string(&order).unwrap();

        // Remove journal_bytes from the JSON to simulate an old DB row.
        let mut value: serde_json::Value = serde_json::from_str(&json).unwrap();
        value.as_object_mut().unwrap().remove("journal_bytes");
        let json_without_field = serde_json::to_string(&value).unwrap();

        // Deserialize and verify journal_bytes defaults to None.
        let deserialized: Order = serde_json::from_str(&json_without_field).unwrap();
        assert!(deserialized.journal_bytes.is_none());

        // Verify that an explicit null also round-trips to None.
        let mut value_null: serde_json::Value = serde_json::from_str(&json).unwrap();
        value_null.as_object_mut().unwrap().insert("journal_bytes".into(), serde_json::Value::Null);
        let json_with_null = serde_json::to_string(&value_null).unwrap();
        let deserialized_null: Order = serde_json::from_str(&json_with_null).unwrap();
        assert!(deserialized_null.journal_bytes.is_none());
    }
}
