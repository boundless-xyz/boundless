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

use crate::{
    config::ConfigLock,
    errors::CodedError,
    impl_coded_debug,
    task::{RetryRes, RetryTask},
};
use alloy::primitives::Address;
use anyhow::Result;
use requestor_lists::{Extensions, RequestorEntry, RequestorList};
use std::{
    collections::HashMap,
    sync::{Arc, RwLock},
};
use thiserror::Error;
use tokio::time::Duration;
use tokio_util::sync::CancellationToken;

const REFRESH_INTERVAL: Duration = Duration::from_secs(3600); // 1 hour

#[derive(Error)]
pub enum MonitorError {
    #[error("Failed to lock internal state")]
    LockFailed,
}

impl_coded_debug!(MonitorError);

impl CodedError for MonitorError {
    fn code(&self) -> &str {
        match self {
            MonitorError::LockFailed => "[B-RM-4001]",
        }
    }
}

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
    config: ConfigLock,
    /// Chain ID for this broker instance
    chain_id: u64,
}

/// Tracks allowed requestors from both static config and remote lists
#[derive(Clone, Debug)]
pub struct AllowRequestors {
    /// Map of requestor addresses to their cached entries (from remote lists)
    requestors: Arc<RwLock<HashMap<Address, CachedEntry>>>,
    /// Config lock for reading latest static addresses
    config: ConfigLock,
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

    /// Update the priority requestors from a list
    fn update_from_list(&self, list: &RequestorList, source_url: &str) {
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
    fn clear_from_url(&self, url: &str) {
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
    fn update_from_list(&self, list: &RequestorList, source_url: &str) {
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
    fn clear_from_url(&self, url: &str) {
        if let Ok(mut requestors) = self.requestors.write() {
            requestors.retain(|_, cached| cached.source_url != url);
        }
    }
}

/// Service for periodically monitoring and refreshing requestor priority and allowed lists
pub struct RequestorMonitor {
    priority_requestors: PriorityRequestors,
    allow_requestors: AllowRequestors,
}

impl RequestorMonitor {
    pub fn new(priority_requestors: PriorityRequestors, allow_requestors: AllowRequestors) -> Self {
        Self { priority_requestors, allow_requestors }
    }

    /// Refresh all configured requestor lists (both priority and allowed)
    async fn refresh_lists(&self) -> Result<()> {
        let (priority_urls, allowed_urls) = {
            let config =
                self.priority_requestors.config.lock_all().map_err(|_| MonitorError::LockFailed)?;
            (
                config.market.priority_requestor_lists.clone(),
                config.market.allow_requestor_lists.clone(),
            )
        };

        // Refresh priority lists
        if let Some(urls) = priority_urls {
            if !urls.is_empty() {
                tracing::debug!("Refreshing {} requestor priority lists", urls.len());

                // Iterate in reverse order so the first list takes precedence
                // (last URL processed overwrites entries from earlier URLs)
                for url in urls.iter().rev() {
                    match RequestorList::fetch_from_url(url).await {
                        Ok(list) => {
                            tracing::info!(
                                "Fetched priority requestor list '{}' (v{}.{}, schema v{}.{}) from {}",
                                list.name,
                                list.version.major,
                                list.version.minor,
                                list.schema_version.major,
                                list.schema_version.minor,
                                url
                            );
                            // Clear entries from this URL before adding new ones
                            self.priority_requestors.clear_from_url(url);
                            self.priority_requestors.update_from_list(&list, url);
                        }
                        Err(e) => {
                            tracing::error!(
                                "Failed to fetch priority requestor list from {}: {}",
                                url,
                                e
                            );
                            // Keep existing cached entries from this URL on failure
                        }
                    }
                }
            }
        }

        // Refresh allowed lists
        if let Some(urls) = allowed_urls {
            if !urls.is_empty() {
                tracing::debug!("Refreshing {} requestor allowed lists", urls.len());

                // Process all lists - order doesn't matter for whitelist (union operation)
                // All addresses from all lists will be allowed
                for url in urls.iter() {
                    match RequestorList::fetch_from_url(url).await {
                        Ok(list) => {
                            tracing::info!(
                                "Fetched allowed requestor list '{}' (v{}.{}, schema v{}.{}) from {}",
                                list.name,
                                list.version.major,
                                list.version.minor,
                                list.schema_version.major,
                                list.schema_version.minor,
                                url
                            );
                            // Clear entries from this URL before adding new ones
                            // (allows updating a list by re-fetching from the same URL)
                            self.allow_requestors.clear_from_url(url);
                            self.allow_requestors.update_from_list(&list, url);
                        }
                        Err(e) => {
                            tracing::error!(
                                "Failed to fetch allowed requestor list from {}: {}",
                                url,
                                e
                            );
                        }
                    }
                }
            }
        }

        Ok(())
    }

    /// Monitor loop for refreshing requestor lists
    async fn monitor_loop(&self, cancel_token: CancellationToken) {
        tracing::info!("Starting requestor list monitor service");

        // Initial refresh
        if let Err(e) = self.refresh_lists().await {
            tracing::error!("Initial requestor list refresh failed: {}", e);
        }

        let mut interval = tokio::time::interval(REFRESH_INTERVAL);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        loop {
            tokio::select! {
                _ = interval.tick() => {
                    if let Err(e) = self.refresh_lists().await {
                        tracing::error!("Failed to refresh requestor lists: {}", e);
                    }
                }
                _ = cancel_token.cancelled() => {
                    tracing::info!("Requestor list monitor shutting down");
                    break;
                }
            }
        }
    }
}

impl RetryTask for RequestorMonitor {
    type Error = MonitorError;

    fn spawn(&self, cancel_token: CancellationToken) -> RetryRes<Self::Error> {
        let priority_requestors = self.priority_requestors.clone();
        let allow_requestors = self.allow_requestors.clone();
        let monitor = Self { priority_requestors, allow_requestors };

        Box::pin(async move {
            monitor.monitor_loop(cancel_token).await;
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ConfigLock;
    use requestor_lists::{
        Extensions, PriorityExtension, RequestEstimatesExtension, RequestorEntry, RequestorList,
        Version,
    };

    fn create_test_config() -> ConfigLock {
        ConfigLock::default()
    }

    fn create_test_entry(
        address: &str,
        chain_id: u64,
        name: &str,
        priority: Option<i32>,
    ) -> RequestorEntry {
        RequestorEntry {
            address: address.parse().unwrap(),
            chain_id,
            name: name.to_string(),
            description: None,
            tags: vec![],
            extensions: Extensions {
                priority: priority.map(|level| PriorityExtension { level }),
                request_estimates: Some(RequestEstimatesExtension {
                    estimated_mcycle_count_min: 100,
                    estimated_mcycle_count_max: 1000,
                    estimated_max_input_size_mb: 10.0,
                }),
                denylist: None,
            },
        }
    }

    #[test]
    fn test_chain_id_filtering() {
        let config = create_test_config();
        let priority_requestors = PriorityRequestors::new(config, 1); // Chain ID 1 (mainnet)

        let list = RequestorList::new(
            "Multi-Chain List".to_string(),
            "Test list with multiple chains".to_string(),
            Version { major: 1, minor: 0 },
            vec![
                create_test_entry(
                    "0xc4ce4f04b9907a9401a0ed7ef073dffebab52aab",
                    1,
                    "Mainnet Requestor",
                    Some(50),
                ),
                create_test_entry(
                    "0x734df7809c4ef94da037449c287166d114503198",
                    8453,
                    "Base Requestor",
                    Some(75),
                ),
                create_test_entry(
                    "0x382bba7d7bc9ae86c5de3e16c4ca96bcc0a3478e",
                    1,
                    "Another Mainnet Requestor",
                    Some(60),
                ),
            ],
        );

        priority_requestors.update_from_list(&list, "https://test.example.com/list.json");

        // Should only have entries from chain_id 1
        let mainnet_addr1: alloy::primitives::Address =
            "0xc4ce4f04b9907a9401a0ed7ef073dffebab52aab".parse().unwrap();
        let base_addr: alloy::primitives::Address =
            "0x734df7809c4ef94da037449c287166d114503198".parse().unwrap();
        let mainnet_addr2: alloy::primitives::Address =
            "0x382bba7d7bc9ae86c5de3e16c4ca96bcc0a3478e".parse().unwrap();

        assert!(priority_requestors.is_priority_requestor(&mainnet_addr1));
        assert!(!priority_requestors.is_priority_requestor(&base_addr)); // Different chain
        assert!(priority_requestors.is_priority_requestor(&mainnet_addr2));
    }

    #[test]
    fn test_get_requestor_entry() {
        let config = create_test_config();
        let priority_requestors = PriorityRequestors::new(config, 1);

        let addr: alloy::primitives::Address =
            "0xc4ce4f04b9907a9401a0ed7ef073dffebab52aab".parse().unwrap();

        let list = RequestorList::new(
            "Test List".to_string(),
            "Test list".to_string(),
            Version { major: 1, minor: 0 },
            vec![create_test_entry(
                "0xc4ce4f04b9907a9401a0ed7ef073dffebab52aab",
                1,
                "Test Requestor",
                Some(75),
            )],
        );

        priority_requestors.update_from_list(&list, "https://test.example.com/list.json");

        let entry = priority_requestors.get_requestor_entry(&addr);
        assert!(entry.is_some());

        let entry = entry.unwrap();
        assert_eq!(entry.name, "Test Requestor");
        assert_eq!(entry.chain_id, 1);
        assert_eq!(entry.extensions.priority.unwrap().level, 75);
        assert!(entry.extensions.request_estimates.is_some());
        assert_eq!(entry.extensions.request_estimates.unwrap().estimated_mcycle_count_min, 100);
    }

    #[test]
    fn test_get_requestor_entry_from_static_config() {
        let config = create_test_config();
        let static_addr: alloy::primitives::Address =
            "0xc4ce4f04b9907a9401a0ed7ef073dffebab52aab".parse().unwrap();

        // Set up static config
        {
            let mut cfg = config.load_write().unwrap();
            cfg.market.priority_requestor_addresses = Some(vec![static_addr]);
        }

        let priority_requestors = PriorityRequestors::new(config, 1);

        let entry = priority_requestors.get_requestor_entry(&static_addr);
        assert!(entry.is_some());

        let entry = entry.unwrap();
        assert_eq!(entry.name, "Static Config Requestor");
        assert_eq!(entry.chain_id, 1);
        assert_eq!(entry.tags, vec!["static"]);
        // Static config entries have no extensions
        assert!(entry.extensions.priority.is_none());
        assert!(entry.extensions.request_estimates.is_none());
    }

    #[test]
    fn test_multiple_lists_precedence() {
        let config = create_test_config();
        let priority_requestors = PriorityRequestors::new(config, 1);

        let addr: alloy::primitives::Address =
            "0xc4ce4f04b9907a9401a0ed7ef073dffebab52aab".parse().unwrap();

        // First list
        let list1 = RequestorList::new(
            "First List".to_string(),
            "First list".to_string(),
            Version { major: 1, minor: 0 },
            vec![create_test_entry(
                "0xc4ce4f04b9907a9401a0ed7ef073dffebab52aab",
                1,
                "First Entry",
                Some(50),
            )],
        );

        // Second list with same address but different data
        let list2 = RequestorList::new(
            "Second List".to_string(),
            "Second list".to_string(),
            Version { major: 1, minor: 0 },
            vec![create_test_entry(
                "0xc4ce4f04b9907a9401a0ed7ef073dffebab52aab",
                1,
                "Second Entry",
                Some(75),
            )],
        );

        // Process first list
        priority_requestors.update_from_list(&list1, "https://test.example.com/list.json");
        let entry = priority_requestors.get_requestor_entry(&addr).unwrap();
        assert_eq!(entry.name, "First Entry");
        assert_eq!(entry.extensions.priority.unwrap().level, 50);

        // Process second list - should overwrite
        priority_requestors.update_from_list(&list2, "https://test.example.com/list.json");
        let entry = priority_requestors.get_requestor_entry(&addr).unwrap();
        assert_eq!(entry.name, "Second Entry");
        assert_eq!(entry.extensions.priority.unwrap().level, 75);
    }

    #[test]
    fn test_first_list_precedence_in_config() {
        let config = create_test_config();
        let priority_requestors = PriorityRequestors::new(config, 1);

        let addr: alloy::primitives::Address =
            "0xc4ce4f04b9907a9401a0ed7ef073dffebab52aab".parse().unwrap();

        // Simulate what refresh_lists does: process in reverse order
        let lists = [
            // First list in config - should have highest precedence
            RequestorList::new(
                "First List".to_string(),
                "First list".to_string(),
                Version { major: 1, minor: 0 },
                vec![create_test_entry(
                    "0xc4ce4f04b9907a9401a0ed7ef073dffebab52aab",
                    1,
                    "First Entry (High Priority)",
                    Some(90),
                )],
            ),
            // Second list in config - should have lower precedence
            RequestorList::new(
                "Second List".to_string(),
                "Second list".to_string(),
                Version { major: 1, minor: 0 },
                vec![create_test_entry(
                    "0xc4ce4f04b9907a9401a0ed7ef073dffebab52aab",
                    1,
                    "Second Entry (Low Priority)",
                    Some(30),
                )],
            ),
        ];

        // Process in reverse order (as refresh_lists does)
        for list in lists.iter().rev() {
            priority_requestors.update_from_list(list, "https://test.example.com/list.json");
        }

        // The first list should win
        let entry = priority_requestors.get_requestor_entry(&addr).unwrap();
        assert_eq!(entry.name, "First Entry (High Priority)");
        assert_eq!(entry.extensions.priority.unwrap().level, 90);
    }

    // Tests for AllowedRequestors (whitelist functionality)

    #[test]
    fn test_allowed_chain_id_filtering() {
        let config = create_test_config();
        let allow_requestors = AllowRequestors::new(config, 1); // Chain ID 1 (mainnet)

        let list = RequestorList::new(
            "Multi-Chain List".to_string(),
            "Test list with multiple chains".to_string(),
            Version { major: 1, minor: 0 },
            vec![
                create_test_entry(
                    "0xc4ce4f04b9907a9401a0ed7ef073dffebab52aab",
                    1,
                    "Mainnet Requestor",
                    None, // No priority needed for allowed list
                ),
                create_test_entry(
                    "0x734df7809c4ef94da037449c287166d114503198",
                    8453,
                    "Base Requestor",
                    None,
                ),
                create_test_entry(
                    "0x382bba7d7bc9ae86c5de3e16c4ca96bcc0a3478e",
                    1,
                    "Another Mainnet Requestor",
                    None,
                ),
            ],
        );

        allow_requestors.update_from_list(&list, "https://test.example.com/list.json");

        // Should only have entries from chain_id 1
        let mainnet_addr1: alloy::primitives::Address =
            "0xc4ce4f04b9907a9401a0ed7ef073dffebab52aab".parse().unwrap();
        let base_addr: alloy::primitives::Address =
            "0x734df7809c4ef94da037449c287166d114503198".parse().unwrap();
        let mainnet_addr2: alloy::primitives::Address =
            "0x382bba7d7bc9ae86c5de3e16c4ca96bcc0a3478e".parse().unwrap();

        assert!(allow_requestors.is_allow_requestor(&mainnet_addr1));
        assert!(!allow_requestors.is_allow_requestor(&base_addr)); // Different chain
        assert!(allow_requestors.is_allow_requestor(&mainnet_addr2));
    }

    #[test]
    fn test_get_allowed_requestor_entry() {
        let config = create_test_config();
        let allow_requestors = AllowRequestors::new(config, 1);

        let addr: alloy::primitives::Address =
            "0xc4ce4f04b9907a9401a0ed7ef073dffebab52aab".parse().unwrap();

        let list = RequestorList::new(
            "Test List".to_string(),
            "Test list".to_string(),
            Version { major: 1, minor: 0 },
            vec![create_test_entry(
                "0xc4ce4f04b9907a9401a0ed7ef073dffebab52aab",
                1,
                "Test Requestor",
                None, // No priority needed for allowed list
            )],
        );

        allow_requestors.update_from_list(&list, "https://test.example.com/list.json");

        let entry = allow_requestors.get_requestor_entry(&addr);
        assert!(entry.is_some());

        let entry = entry.unwrap();
        assert_eq!(entry.name, "Test Requestor");
        assert_eq!(entry.chain_id, 1);
        // Allowed list entries don't need priority extensions
    }

    #[test]
    fn test_get_allowed_requestor_entry_from_static_config() {
        let config = create_test_config();
        let static_addr: alloy::primitives::Address =
            "0xc4ce4f04b9907a9401a0ed7ef073dffebab52aab".parse().unwrap();

        // Set up static config
        {
            let mut cfg = config.load_write().unwrap();
            cfg.market.allow_client_addresses = Some(vec![static_addr]);
        }

        let allow_requestors = AllowRequestors::new(config, 1);

        let entry = allow_requestors.get_requestor_entry(&static_addr);
        assert!(entry.is_some());

        let entry = entry.unwrap();
        assert_eq!(entry.name, "Static Config Requestor");
        assert_eq!(entry.chain_id, 1);
        assert_eq!(entry.tags, vec!["static"]);
        // Static config entries have no extensions
        assert!(entry.extensions.priority.is_none());
        assert!(entry.extensions.request_estimates.is_none());
    }

    #[test]
    fn test_allowed_multiple_lists_union() {
        let config = create_test_config();
        let allow_requestors = AllowRequestors::new(config, 1);

        let addr1: alloy::primitives::Address =
            "0xc4ce4f04b9907a9401a0ed7ef073dffebab52aab".parse().unwrap();
        let addr2: alloy::primitives::Address =
            "0x734df7809c4ef94da037449c287166d114503198".parse().unwrap();
        let addr3: alloy::primitives::Address =
            "0x382bba7d7bc9ae86c5de3e16c4ca96bcc0a3478e".parse().unwrap();

        // First list with addresses 1 and 2
        let list1 = RequestorList::new(
            "First List".to_string(),
            "First list".to_string(),
            Version { major: 1, minor: 0 },
            vec![
                create_test_entry(
                    "0xc4ce4f04b9907a9401a0ed7ef073dffebab52aab",
                    1,
                    "First Entry",
                    None,
                ),
                create_test_entry(
                    "0x734df7809c4ef94da037449c287166d114503198",
                    1,
                    "Second Entry",
                    None,
                ),
            ],
        );

        // Second list with addresses 2 and 3 (address 2 overlaps)
        let list2 = RequestorList::new(
            "Second List".to_string(),
            "Second list".to_string(),
            Version { major: 1, minor: 0 },
            vec![
                create_test_entry(
                    "0x734df7809c4ef94da037449c287166d114503198",
                    1,
                    "Second Entry (Updated)",
                    None,
                ),
                create_test_entry(
                    "0x382bba7d7bc9ae86c5de3e16c4ca96bcc0a3478e",
                    1,
                    "Third Entry",
                    None,
                ),
            ],
        );

        // Process first list
        allow_requestors.update_from_list(&list1, "https://test.example.com/list1.json");
        assert!(allow_requestors.is_allow_requestor(&addr1));
        assert!(allow_requestors.is_allow_requestor(&addr2));
        assert!(!allow_requestors.is_allow_requestor(&addr3));

        // Process second list - should create union (all addresses from both lists allowed)
        allow_requestors.update_from_list(&list2, "https://test.example.com/list2.json");

        // All addresses from both lists should be allowed (union behavior)
        assert!(allow_requestors.is_allow_requestor(&addr1), "addr1 from list1 should be allowed");
        assert!(
            allow_requestors.is_allow_requestor(&addr2),
            "addr2 from both lists should be allowed"
        );
        assert!(allow_requestors.is_allow_requestor(&addr3), "addr3 from list2 should be allowed");

        // Address 2 appears in both lists - metadata from last processed list should be used
        let entry = allow_requestors.get_requestor_entry(&addr2).unwrap();
        assert_eq!(entry.name, "Second Entry (Updated)");
    }

    #[test]
    fn test_allowed_multiple_lists_order_independent() {
        let config = create_test_config();
        let allow_requestors = AllowRequestors::new(config.clone(), 1);

        let addr1: alloy::primitives::Address =
            "0xc4ce4f04b9907a9401a0ed7ef073dffebab52aab".parse().unwrap();
        let addr2: alloy::primitives::Address =
            "0x734df7809c4ef94da037449c287166d114503198".parse().unwrap();

        let list1 = RequestorList::new(
            "List 1".to_string(),
            "First list".to_string(),
            Version { major: 1, minor: 0 },
            vec![create_test_entry(
                "0xc4ce4f04b9907a9401a0ed7ef073dffebab52aab",
                1,
                "Entry 1",
                None,
            )],
        );

        let list2 = RequestorList::new(
            "List 2".to_string(),
            "Second list".to_string(),
            Version { major: 1, minor: 0 },
            vec![create_test_entry(
                "0x734df7809c4ef94da037449c287166d114503198",
                1,
                "Entry 2",
                None,
            )],
        );

        // Process lists in one order
        allow_requestors.update_from_list(&list1, "https://test.example.com/list1.json");
        allow_requestors.update_from_list(&list2, "https://test.example.com/list2.json");

        assert!(allow_requestors.is_allow_requestor(&addr1));
        assert!(allow_requestors.is_allow_requestor(&addr2));

        // Clear and process in reverse order - result should be the same (union)
        let allow_requestors2 = AllowRequestors::new(config, 1);
        allow_requestors2.update_from_list(&list2, "https://test.example.com/list2.json");
        allow_requestors2.update_from_list(&list1, "https://test.example.com/list1.json");

        assert!(
            allow_requestors2.is_allow_requestor(&addr1),
            "Order should not matter - addr1 should be allowed"
        );
        assert!(
            allow_requestors2.is_allow_requestor(&addr2),
            "Order should not matter - addr2 should be allowed"
        );
    }

    #[test]
    fn test_allowed_list_whitelist_behavior() {
        let config = create_test_config();
        let allow_requestors = AllowRequestors::new(config, 1);

        let allowed_addr: alloy::primitives::Address =
            "0xc4ce4f04b9907a9401a0ed7ef073dffebab52aab".parse().unwrap();
        let not_allowed_addr: alloy::primitives::Address =
            "0x734df7809c4ef94da037449c287166d114503198".parse().unwrap();

        let list = RequestorList::new(
            "Allowed List".to_string(),
            "Test allowed list".to_string(),
            Version { major: 1, minor: 0 },
            vec![create_test_entry(
                "0xc4ce4f04b9907a9401a0ed7ef073dffebab52aab",
                1,
                "Allowed Requestor",
                None,
            )],
        );

        allow_requestors.update_from_list(&list, "https://test.example.com/list.json");

        // Address in list should be allowed
        assert!(allow_requestors.is_allow_requestor(&allowed_addr));

        // Address not in list should not be allowed
        assert!(!allow_requestors.is_allow_requestor(&not_allowed_addr));
    }
}
