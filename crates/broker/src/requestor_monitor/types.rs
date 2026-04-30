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

//! [`PriorityRequestors`] and [`AllowRequestors`] — query-side handles for
//! the requestor lists. Both are shared across services (held by the
//! [`OrderPricer`](crate::order_pricer::OrderPricer),
//! [`OrderEvaluator`](crate::order_evaluator::OrderEvaluator), etc.) and
//! refreshed by [`RequestorMonitor`](super::RequestorMonitor).

use std::{
    collections::HashMap,
    sync::{Arc, RwLock},
};

use alloy::primitives::Address;
use requestor_lists::{Extensions, RequestorEntry, RequestorList};

use crate::config::ConfigLock;

/// Cached entry that tracks both the requestor data and its source URL
#[derive(Clone, Debug)]
struct CachedEntry {
    entry: RequestorEntry,
    source_url: String,
}

/// Tracks priority requestors from both static config and remote lists
#[derive(Clone, Debug)]
pub struct PriorityRequestors {
    /// Map of requestor addresses to their cached entries (from remote lists)
    requestors: Arc<RwLock<HashMap<Address, CachedEntry>>>,
    /// Config lock for reading latest static addresses
    pub(super) config: ConfigLock,
    /// Chain ID for this broker instance
    chain_id: u64,
}

/// Tracks allowed requestors from both static config and remote lists
#[derive(Clone, Debug)]
pub struct AllowRequestors {
    /// Map of requestor addresses to their cached entries (from remote lists)
    requestors: Arc<RwLock<HashMap<Address, CachedEntry>>>,
    /// Config lock for reading latest static addresses
    pub(super) config: ConfigLock,
    /// Chain ID for this broker instance
    chain_id: u64,
}

impl PriorityRequestors {
    pub fn new(config: ConfigLock, chain_id: u64) -> Self {
        Self { requestors: Arc::new(RwLock::new(HashMap::new())), config, chain_id }
    }

    /// Get requestor entry for an address, checking both remote lists and static config
    pub fn get_requestor_entry(&self, address: &Address) -> Option<RequestorEntry> {
        // First check cached remote list entries
        if let Ok(requestors) = self.requestors.read() {
            if let Some(cached) = requestors.get(address) {
                return Some(cached.entry.clone());
            }
        }

        // Then check static config addresses
        if let Ok(config) = self.config.lock_all() {
            if let Some(static_addresses) = &config.market.priority_requestor_addresses {
                if static_addresses.contains(address) {
                    // Create a default entry for static config addresses (no extensions)
                    return Some(RequestorEntry {
                        address: *address,
                        chain_id: self.chain_id,
                        name: "Static Config Requestor".to_string(),
                        description: None,
                        tags: vec!["static".to_string()],
                        extensions: Extensions::default(),
                    });
                }
            }
        }

        None
    }

    /// Check if an address is a priority requestor
    pub fn is_priority_requestor(&self, address: &Address) -> bool {
        self.get_requestor_entry(address).is_some()
    }

    /// Returns all dynamically-registered priority addresses (from remote lists).
    pub fn dynamic_addresses(&self) -> Vec<Address> {
        self.requestors.read().map(|r| r.keys().copied().collect()).unwrap_or_default()
    }

    /// Update the priority requestors from a list
    pub(super) fn update_from_list(&self, list: &RequestorList, source_url: &str) {
        let mut requestors = match self.requestors.write() {
            Ok(r) => r,
            Err(e) => {
                tracing::error!("Failed to lock requestors for writing: {e:?}");
                return;
            }
        };

        let mut added_count = 0;
        for entry in &list.requestors {
            // Only add requestors for the chain we're operating on
            if entry.chain_id == self.chain_id {
                requestors.insert(
                    entry.address,
                    CachedEntry { entry: entry.clone(), source_url: source_url.to_string() },
                );
                added_count += 1;
            }
        }

        tracing::info!(
            "Updated priority requestors from list '{}' (v{}.{}, schema v{}.{}): {} entries added (chain_id={}, {} total in list)",
            list.name,
            list.version.major,
            list.version.minor,
            list.schema_version.major,
            list.schema_version.minor,
            added_count,
            self.chain_id,
            list.requestors.len()
        );
    }

    /// Clear cached priority requestors from a specific URL
    pub(super) fn clear_from_url(&self, url: &str) {
        if let Ok(mut requestors) = self.requestors.write() {
            requestors.retain(|_, cached| cached.source_url != url);
        }
    }
}

impl AllowRequestors {
    pub fn new(config: ConfigLock, chain_id: u64) -> Self {
        Self { requestors: Arc::new(RwLock::new(HashMap::new())), config, chain_id }
    }

    /// Get requestor entry for an address, checking both remote lists and static config
    pub fn get_requestor_entry(&self, address: &Address) -> Option<RequestorEntry> {
        // First check cached remote list entries
        if let Ok(requestors) = self.requestors.read() {
            if let Some(cached) = requestors.get(address) {
                return Some(cached.entry.clone());
            }
        }

        // Then check static config addresses
        if let Ok(config) = self.config.lock_all() {
            if let Some(static_addresses) = &config.market.allow_client_addresses {
                if static_addresses.contains(address) {
                    // Create a default entry for static config addresses (no extensions)
                    return Some(RequestorEntry {
                        address: *address,
                        chain_id: self.chain_id,
                        name: "Static Config Requestor".to_string(),
                        description: None,
                        tags: vec!["static".to_string()],
                        extensions: Extensions::default(),
                    });
                }
            }
        }

        None
    }

    /// Check if an address is an allowed requestor
    pub fn is_allow_requestor(&self, address: &Address) -> bool {
        self.get_requestor_entry(address).is_some()
    }

    /// Update the allow requestors from a list
    pub(super) fn update_from_list(&self, list: &RequestorList, source_url: &str) {
        let mut requestors = match self.requestors.write() {
            Ok(r) => r,
            Err(e) => {
                tracing::error!("Failed to lock requestors for writing: {e:?}");
                return;
            }
        };

        let mut added_count = 0;
        for entry in &list.requestors {
            // Only add requestors for the chain we're operating on
            if entry.chain_id == self.chain_id {
                requestors.insert(
                    entry.address,
                    CachedEntry { entry: entry.clone(), source_url: source_url.to_string() },
                );
                added_count += 1;
            }
        }

        tracing::info!(
            "Updated allowed requestors from list '{}' (v{}.{}, schema v{}.{}): {} entries added (chain_id={}, {} total in list)",
            list.name,
            list.version.major,
            list.version.minor,
            list.schema_version.major,
            list.schema_version.minor,
            added_count,
            self.chain_id,
            list.requestors.len()
        );
    }

    /// Clear cached allowed requestors from a specific URL
    pub(super) fn clear_from_url(&self, url: &str) {
        if let Ok(mut requestors) = self.requestors.write() {
            requestors.retain(|_, cached| cached.source_url != url);
        }
    }
}
