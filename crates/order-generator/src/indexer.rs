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

//! Indexer-based withdrawal safety check for address rotation.
//!
//! Defines the [`RequestorIndexer`] trait (mockable in tests) and implements it
//! for [`IndexerClient`] by paging through all results. The two pure helpers
//! [`is_safe_to_withdraw`] and [`next_safe_at`] contain the core decision logic.
//!
//! ## Safety semantics
//!
//! | Status      | Safe to withdraw?                           |
//! |-------------|---------------------------------------------|
//! | `fulfilled` | Always (payment already made)               |
//! | `expired`   | Always (escrow returned)                    |
//! | `locked`    | Always (balance debited at lock time)       |
//! | `submitted` | After `now > lock_end` (locking window closed) |

use alloy::primitives::Address;
use async_trait::async_trait;
use boundless_market::indexer_client::{IndexerClient, RequestorRequestsParams};

/// Parsed status of a single request, sufficient to decide if withdrawal is safe.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RequestStatus {
    Submitted,
    Locked,
    Fulfilled,
    Expired,
}

/// Minimal summary of a request needed for the withdrawal safety check.
#[derive(Debug, Clone)]
pub struct RequestSummary {
    /// Parsed request status.
    pub status: RequestStatus,
    /// Unix timestamp when the locking window closes.
    /// For `submitted` requests: safe to withdraw once `now > lock_end`.
    pub lock_end: u64,
}

/// Abstraction over the indexer for withdrawal safety checks.
///
/// Implement this trait with a mock in unit tests to avoid needing a live indexer.
#[async_trait]
pub trait RequestorIndexer: Send + Sync {
    /// Returns all requests submitted by `address`, across all pages.
    async fn list_requests(&self, address: Address) -> anyhow::Result<Vec<RequestSummary>>;
}

/// Production implementation: pages through the indexer API (limit=500 per page).
#[async_trait]
impl RequestorIndexer for IndexerClient {
    async fn list_requests(&self, address: Address) -> anyhow::Result<Vec<RequestSummary>> {
        let mut all = Vec::new();
        let mut cursor = None;
        loop {
            let page = self
                .list_requests_by_requestor(
                    address,
                    RequestorRequestsParams { cursor, limit: Some(500), sort_by: None },
                )
                .await?;
            for r in page.data {
                let status = match r.request_status.as_str() {
                    "submitted" => RequestStatus::Submitted,
                    "locked" => RequestStatus::Locked,
                    "fulfilled" => RequestStatus::Fulfilled,
                    _ => RequestStatus::Expired,
                };
                all.push(RequestSummary { status, lock_end: r.lock_end as u64 });
            }
            if page.has_more {
                cursor = page.next_cursor;
            } else {
                break;
            }
        }
        Ok(all)
    }
}

/// Returns `true` when it is safe to withdraw from an address given its current requests.
///
/// Safe when every request is either terminal (`fulfilled`/`expired`/`locked`) or a
/// `submitted` request whose locking window has already closed (`now > lock_end`).
pub fn is_safe_to_withdraw(requests: &[RequestSummary], now: u64) -> bool {
    requests.iter().all(|r| r.status != RequestStatus::Submitted || now > r.lock_end)
}

/// Returns the earliest timestamp at which [`is_safe_to_withdraw`] will return `true`,
/// i.e. the maximum `lock_end` across all `submitted` requests. Returns `0` if already safe.
pub fn next_safe_at(requests: &[RequestSummary]) -> u64 {
    requests
        .iter()
        .filter(|r| r.status == RequestStatus::Submitted)
        .map(|r| r.lock_end)
        .max()
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct MockIndexer {
        requests: Vec<RequestSummary>,
    }

    #[async_trait]
    impl RequestorIndexer for MockIndexer {
        async fn list_requests(&self, _address: Address) -> anyhow::Result<Vec<RequestSummary>> {
            Ok(self.requests.clone())
        }
    }

    fn summary(status: RequestStatus, lock_end: u64) -> RequestSummary {
        RequestSummary { status, lock_end }
    }

    #[test]
    fn empty_is_safe() {
        assert!(is_safe_to_withdraw(&[], 1000));
    }

    #[test]
    fn all_fulfilled_is_safe() {
        let reqs =
            vec![summary(RequestStatus::Fulfilled, 0), summary(RequestStatus::Fulfilled, 9999)];
        assert!(is_safe_to_withdraw(&reqs, 500));
    }

    #[test]
    fn all_expired_is_safe() {
        let reqs = vec![summary(RequestStatus::Expired, 0)];
        assert!(is_safe_to_withdraw(&reqs, 1));
    }

    #[test]
    fn locked_is_always_safe() {
        // locked requests have balance already settled at lock time
        let reqs = vec![summary(RequestStatus::Locked, 9999)];
        assert!(is_safe_to_withdraw(&reqs, 0));
    }

    #[test]
    fn submitted_before_lock_end_is_not_safe() {
        let reqs = vec![summary(RequestStatus::Submitted, 1000)];
        assert!(!is_safe_to_withdraw(&reqs, 999));
        assert!(!is_safe_to_withdraw(&reqs, 1000)); // boundary: must be strictly greater
    }

    #[test]
    fn submitted_after_lock_end_is_safe() {
        let reqs = vec![summary(RequestStatus::Submitted, 1000)];
        assert!(is_safe_to_withdraw(&reqs, 1001));
    }

    #[test]
    fn mix_fulfilled_and_submitted_pending() {
        let reqs =
            vec![summary(RequestStatus::Fulfilled, 0), summary(RequestStatus::Submitted, 2000)];
        assert!(!is_safe_to_withdraw(&reqs, 1999));
        assert!(is_safe_to_withdraw(&reqs, 2001));
    }

    #[test]
    fn next_safe_at_no_submitted() {
        let reqs = vec![summary(RequestStatus::Fulfilled, 500)];
        assert_eq!(next_safe_at(&reqs), 0);
    }

    #[test]
    fn next_safe_at_returns_max_lock_end() {
        let reqs = vec![
            summary(RequestStatus::Submitted, 500),
            summary(RequestStatus::Submitted, 1500),
            summary(RequestStatus::Fulfilled, 9999),
        ];
        assert_eq!(next_safe_at(&reqs), 1500);
    }

    #[tokio::test]
    async fn mock_indexer_returns_requests() {
        let indexer = MockIndexer {
            requests: vec![
                summary(RequestStatus::Submitted, 1000),
                summary(RequestStatus::Fulfilled, 0),
            ],
        };
        let addr = Address::ZERO;
        let reqs = indexer.list_requests(addr).await.unwrap();
        assert_eq!(reqs.len(), 2);
        assert!(!is_safe_to_withdraw(&reqs, 999));
        assert!(is_safe_to_withdraw(&reqs, 1001));
    }
}
