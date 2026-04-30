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

//! Internal data types for the order pricer service.
//!
//! [`ActivePreflights`] tracks in-flight preflight tasks so they can be
//! cancelled when their request transitions to a terminal state (locked or
//! fulfilled). [`OrderCache`] is the dedup cache that prevents the same order
//! from being preflighted twice.

use std::collections::BTreeMap;
use std::sync::Arc;

use alloy::primitives::U256;
use moka::future::Cache;
use tokio_util::sync::CancellationToken;

use crate::{FulfillmentType, OrderRequest};

/// Maximum number of orders to cache for deduplication
pub(crate) const ORDER_DEDUP_CACHE_SIZE: u64 = 5000;

/// In-memory LRU cache for order deduplication by ID (prevents duplicate order processing)
pub(crate) type OrderCache = Arc<Cache<String, ()>>;

/// Configuration for preflight result caching
pub(crate) const PREFLIGHT_CACHE_SIZE: u64 = 5000;
pub(crate) const PREFLIGHT_CACHE_TTL_SECS: u64 = 3 * 60 * 60; // 3 hours

/// A preflight task that can be cancelled, tagged with its fulfillment type.
struct PreflightHandle {
    cancel_token: CancellationToken,
    fulfillment_type: FulfillmentType,
}

/// Per-request map of in-flight preflight tasks: request_id → { order_id → task }
#[derive(Default)]
pub(crate) struct ActivePreflights(BTreeMap<U256, BTreeMap<String, PreflightHandle>>);

impl ActivePreflights {
    pub(crate) fn insert(&mut self, order: &OrderRequest, cancel_token: CancellationToken) {
        let request_id = U256::from(order.request.id);
        self.0.entry(request_id).or_default().insert(
            order.id(),
            PreflightHandle { cancel_token, fulfillment_type: order.fulfillment_type },
        );
    }

    pub(crate) fn remove(&mut self, request_id: &U256, order_id: &str) {
        if let Some(tasks) = self.0.get_mut(request_id) {
            tasks.remove(order_id);
            if tasks.is_empty() {
                self.0.remove(request_id);
            }
        }
    }

    pub(crate) fn contains(&self, request_id: &U256, order_id: &str) -> bool {
        self.0.get(request_id).is_some_and(|t| t.contains_key(order_id))
    }

    /// Cancel and remove LockAndFulfill tasks for a locked request.
    pub(crate) fn cancel_lock_and_fulfill(&mut self, request_id: &U256) {
        let Some(tasks) = self.0.get_mut(request_id) else { return };

        let before = tasks.len();
        tasks.retain(|_, task| {
            let should_cancel = task.fulfillment_type == FulfillmentType::LockAndFulfill;
            if should_cancel {
                task.cancel_token.cancel();
            }
            !should_cancel
        });

        let cancelled = before - tasks.len();
        if cancelled > 0 {
            tracing::debug!(
                "Cancelled {cancelled} LockAndFulfill preflights for locked request 0x{request_id:x}"
            );
        }
        if tasks.is_empty() {
            self.0.remove(request_id);
        }
    }

    /// Cancel and remove all tasks for a fulfilled request.
    pub(crate) fn cancel_all(&mut self, request_id: &U256) {
        let Some(tasks) = self.0.remove(request_id) else { return };

        tracing::debug!(
            "Cancelling {} active preflights for fulfilled request 0x{request_id:x}",
            tasks.len(),
        );
        for (_, task) in tasks {
            task.cancel_token.cancel();
        }
    }

    pub(crate) fn format(&self) -> String {
        format_truncated(self.0.values().flat_map(|orders| orders.keys()))
    }
}

/// Format orders for logging, limiting to first 3 and showing total count
fn format_truncated<I, S>(mut iter: I) -> String
where
    I: Iterator<Item = S>,
    S: AsRef<str>,
{
    let first_three: Vec<String> = iter.by_ref().take(3).map(|s| s.as_ref().to_string()).collect();
    let remaining_count = iter.count();

    if remaining_count == 0 {
        first_three.join(", ")
    } else {
        format!("{}, ... ({} total)", first_three.join(", "), first_three.len() + remaining_count)
    }
}
