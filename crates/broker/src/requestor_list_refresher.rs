use crate::{config::ConfigLock, errors::CodedError, impl_coded_debug};
use alloy::primitives::Address;
use anyhow::Result;
use requestor_lists::RequestorList;
use std::{
    collections::HashMap,
    sync::{Arc, RwLock},
};
use thiserror::Error;
use tokio::{task::JoinHandle, time::Duration};
use tokio_util::sync::CancellationToken;

const REFRESH_INTERVAL: Duration = Duration::from_secs(3600); // 1 hour

#[derive(Error)]
pub enum RefresherError {
    #[error("Failed to fetch requestor list from URL: {0}")]
    FetchError(String),
    #[error("Failed to lock internal state")]
    LockFailed,
}

impl_coded_debug!(RefresherError);

impl CodedError for RefresherError {
    fn code(&self) -> &str {
        match self {
            RefresherError::FetchError(_) => "[B-RLR-4001]",
            RefresherError::LockFailed => "[B-RLR-4002]",
        }
    }
}

/// Tracks priority requestors loaded from remote lists
#[derive(Clone, Debug, Default)]
pub struct PriorityRequestors {
    /// Map of requestor addresses to their priority levels
    requestors: Arc<RwLock<HashMap<Address, i32>>>,
}

impl PriorityRequestors {
    pub fn new() -> Self {
        Self { requestors: Arc::new(RwLock::new(HashMap::new())) }
    }

    /// Get the priority for a requestor address
    pub fn get_priority(&self, address: &Address) -> Option<i32> {
        self.requestors.read().ok()?.get(address).copied()
    }

    /// Check if an address is a priority requestor
    pub fn is_priority_requestor(&self, address: &Address) -> bool {
        self.requestors.read().ok().map(|r| r.contains_key(address)).unwrap_or(false)
    }

    /// Check if an address is a priority requestor, combining static config and dynamic list
    pub fn is_priority_requestor_combined(
        &self,
        address: &Address,
        static_addresses: &Option<Vec<Address>>,
    ) -> bool {
        // Check static config list
        if let Some(addresses) = static_addresses {
            if addresses.contains(address) {
                return true;
            }
        }

        // Check dynamic list
        self.is_priority_requestor(address)
    }

    /// Update the priority requestors from a list
    fn update_from_list(&self, list: &RequestorList) {
        let mut requestors = match self.requestors.write() {
            Ok(r) => r,
            Err(e) => {
                tracing::error!("Failed to lock requestors for writing: {e:?}");
                return;
            }
        };

        for entry in &list.requestors {
            requestors.insert(entry.address, entry.priority);
        }

        tracing::info!(
            "Updated priority requestors from list '{}': {} entries",
            list.name,
            list.requestors.len()
        );
    }

    /// Clear all priority requestors
    fn clear(&self) {
        if let Ok(mut requestors) = self.requestors.write() {
            requestors.clear();
        }
    }
}

/// Service for periodically refreshing requestor priority lists
pub struct RequestorListRefresher {
    config: ConfigLock,
    priority_requestors: PriorityRequestors,
    cancel_token: CancellationToken,
}

impl RequestorListRefresher {
    pub fn new(
        config: ConfigLock,
        priority_requestors: PriorityRequestors,
        cancel_token: CancellationToken,
    ) -> Self {
        Self { config, priority_requestors, cancel_token }
    }

    /// Refresh all configured requestor lists
    async fn refresh_lists(&self) -> Result<()> {
        let urls = {
            let config = self.config.lock_all().map_err(|_| RefresherError::LockFailed)?;
            config.market.priority_requestor_lists.clone()
        };

        let Some(urls) = urls else {
            return Ok(());
        };

        if urls.is_empty() {
            return Ok(());
        }

        tracing::debug!("Refreshing {} requestor priority lists", urls.len());

        // Clear existing entries before refreshing
        self.priority_requestors.clear();

        for url in &urls {
            match RequestorList::fetch_from_url(url).await {
                Ok(list) => {
                    self.priority_requestors.update_from_list(&list);
                }
                Err(e) => {
                    tracing::error!("Failed to fetch requestor list from {}: {}", url, e);
                }
            }
        }

        Ok(())
    }

    /// Start the refresher service
    pub fn spawn(self: Arc<Self>) -> JoinHandle<Result<()>> {
        tokio::spawn(async move {
            tracing::info!("Starting requestor list refresher service");

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
                    _ = self.cancel_token.cancelled() => {
                        tracing::info!("Requestor list refresher shutting down");
                        break;
                    }
                }
            }

            Ok(())
        })
    }
}
