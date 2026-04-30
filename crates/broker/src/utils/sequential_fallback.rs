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

//! A sequential transport fallback layer for alloy RPC providers.
//!
//! Unlike alloy's [`FallbackLayer`] which queries transports in parallel,
//! this layer always tries the primary (first) transport first and only falls back
//! to subsequent transports on failure. This minimizes RPC usage on paid endpoints.
//!
//! [`FallbackLayer`]: alloy::transports::layers::FallbackLayer

use std::{
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, RwLock,
    },
    task::{Context, Poll},
};

use alloy::{
    rpc::json_rpc::{RequestPacket, ResponsePacket},
    transports::{TransportError, TransportErrorKind, TransportFut},
};
use tower::{Layer, Service};

/// Number of consecutive failures before a transport is skipped. When a transport fails this many
/// times in a row, it is temporarily skipped to avoid wasting time on an unhealthy endpoint.
const MAX_CONSECUTIVE_FAILURES: u32 = 3;

/// How often to retry a skipped transport (every N-th request).
const RETRY_INTERVAL: u64 = 10;

#[derive(Debug, Default)]
struct TransportHealth {
    consecutive_failures: u32,
    is_skipped: bool,
}

/// A transport with associated health tracking.
#[derive(Debug, Clone)]
struct TrackedTransport<S> {
    transport: S,
    id: usize,
    health: Arc<RwLock<TransportHealth>>,
}

impl<S: Clone> TrackedTransport<S> {
    fn new(id: usize, transport: S) -> Self {
        Self { id, transport, health: Arc::new(RwLock::new(TransportHealth::default())) }
    }

    fn should_try(&self, request_num: u64) -> bool {
        let health = self.health.read().unwrap();
        if !health.is_skipped {
            return true;
        }
        // Periodically retry skipped transports to detect recovery.
        request_num % RETRY_INTERVAL == 0
    }

    fn record_success(&self) {
        let mut health = self.health.write().unwrap();
        if health.is_skipped {
            tracing::info!(transport_id = self.id, "Transport recovered, re-enabling");
        }
        health.consecutive_failures = 0;
        health.is_skipped = false;
    }

    fn record_failure(&self) {
        let mut health = self.health.write().unwrap();
        health.consecutive_failures += 1;
        if health.consecutive_failures >= MAX_CONSECUTIVE_FAILURES && !health.is_skipped {
            tracing::warn!(
                transport_id = self.id,
                consecutive_failures = health.consecutive_failures,
                "Skipping transport after too many consecutive failures"
            );
            health.is_skipped = true;
        }
    }
}

// ── Layer ───────────────────────────────────────────────────────

/// A fallback layer that queries transports sequentially, primary first.
///
/// This is a cost-saving alternative to alloy's [`FallbackLayer`] which queries
/// transports in parallel. By always preferring the primary transport and only
/// falling back when it fails, this layer minimizes RPC usage on paid endpoints.
///
/// If a transport fails a configured number of times in a row it is temporarily skipped.
/// Skipped transports are retried every N-th request to detect recovery.
///
/// [`FallbackLayer`]: alloy::transports::layers::FallbackLayer
#[derive(Debug, Clone, Default)]
pub struct SequentialFallbackLayer;

impl<S> Layer<Vec<S>> for SequentialFallbackLayer
where
    S: Service<RequestPacket, Future = TransportFut<'static>, Error = TransportError>
        + Send
        + Clone
        + 'static,
{
    type Service = SequentialFallbackService<S>;

    fn layer(&self, transports: Vec<S>) -> Self::Service {
        let tracked = transports
            .into_iter()
            .enumerate()
            .map(|(id, t)| TrackedTransport::new(id, t))
            .collect();
        SequentialFallbackService {
            transports: Arc::new(tracked),
            request_counter: Arc::new(AtomicU64::new(0)),
        }
    }
}

// ── Service ─────────────────────────────────────────────────────

/// The service produced by [`SequentialFallbackLayer`].
#[derive(Debug, Clone)]
pub struct SequentialFallbackService<S> {
    transports: Arc<Vec<TrackedTransport<S>>>,
    request_counter: Arc<AtomicU64>,
}

impl<S> SequentialFallbackService<S>
where
    S: Service<RequestPacket, Future = TransportFut<'static>, Error = TransportError>
        + Send
        + Clone
        + 'static,
{
    async fn make_request(&self, req: RequestPacket) -> Result<ResponsePacket, TransportError> {
        let request_num = self.request_counter.fetch_add(1, Ordering::Relaxed);
        let mut last_error = None;
        let mut attempted_any = false;

        for tracked in self.transports.iter() {
            if !tracked.should_try(request_num) {
                tracing::debug!(transport_id = tracked.id, "Skipping transport (unhealthy)");
                continue;
            }

            attempted_any = true;
            let mut transport = tracked.transport.clone();
            match transport.call(req.clone()).await {
                Ok(response) => {
                    tracked.record_success();
                    if tracked.id > 0 {
                        tracing::debug!(
                            transport_id = tracked.id,
                            "Request succeeded on fallback transport"
                        );
                    }
                    return Ok(response);
                }
                Err(err) => {
                    tracked.record_failure();
                    tracing::warn!(
                        transport_id = tracked.id,
                        error = %err,
                        "Transport failed, trying next"
                    );
                    last_error = Some(err);
                }
            }
        }

        // If all transports were in the skip window, retry the primary as a last resort
        // so we never silently drop requests without attempting at least one transport.
        if !attempted_any {
            if let Some(primary) = self.transports.first() {
                tracing::debug!(
                    transport_id = primary.id,
                    "All transports skipped, retrying primary as last resort"
                );
                let mut transport = primary.transport.clone();
                match transport.call(req).await {
                    Ok(response) => {
                        primary.record_success();
                        return Ok(response);
                    }
                    Err(err) => {
                        primary.record_failure();
                        last_error = Some(err);
                    }
                }
            }
        }

        Err(last_error.unwrap_or_else(|| {
            TransportErrorKind::custom_str("All transports failed or are skipped")
        }))
    }
}

impl<S> Service<RequestPacket> for SequentialFallbackService<S>
where
    S: Service<RequestPacket, Future = TransportFut<'static>, Error = TransportError>
        + Send
        + Sync
        + Clone
        + 'static,
{
    type Response = ResponsePacket;
    type Error = TransportError;
    type Future = TransportFut<'static>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, req: RequestPacket) -> Self::Future {
        let this = self.clone();
        Box::pin(async move { this.make_request(req).await })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::transports::layers::{RetryBackoffLayer, RetryPolicy};
    use std::sync::{
        atomic::{AtomicBool, AtomicU32},
        Mutex,
    };
    use tower::{Layer, ServiceBuilder};

    fn dummy_request() -> RequestPacket {
        use alloy::rpc::json_rpc::{Id, Request};
        let req: Request<()> = Request::new("eth_blockNumber", Id::Number(1), ());
        RequestPacket::Single(req.serialize().unwrap())
    }

    fn mock_response() -> ResponsePacket {
        serde_json::from_str(r#"{"jsonrpc":"2.0","id":1,"result":"0x1"}"#).unwrap()
    }

    #[derive(Clone)]
    struct MockTransport {
        call_count: Arc<AtomicU32>,
        should_succeed: Arc<AtomicBool>,
    }

    impl MockTransport {
        fn new(should_succeed: bool) -> Self {
            Self {
                call_count: Arc::new(AtomicU32::new(0)),
                should_succeed: Arc::new(AtomicBool::new(should_succeed)),
            }
        }

        fn count(&self) -> u32 {
            self.call_count.load(Ordering::SeqCst)
        }

        fn set_succeed(&self, value: bool) {
            self.should_succeed.store(value, Ordering::SeqCst);
        }
    }

    impl Service<RequestPacket> for MockTransport {
        type Response = ResponsePacket;
        type Error = TransportError;
        type Future = TransportFut<'static>;

        fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
            Poll::Ready(Ok(()))
        }

        fn call(&mut self, _req: RequestPacket) -> Self::Future {
            self.call_count.fetch_add(1, Ordering::SeqCst);
            let succeed = self.should_succeed.load(Ordering::SeqCst);
            Box::pin(async move {
                if succeed {
                    Ok(mock_response())
                } else {
                    Err(TransportErrorKind::custom_str("mock failure"))
                }
            })
        }
    }

    #[tokio::test]
    async fn uses_primary_transport_on_success() {
        let primary = MockTransport::new(true);
        let secondary = MockTransport::new(true);

        let mut service = SequentialFallbackLayer.layer(vec![primary.clone(), secondary.clone()]);

        let result = service.call(dummy_request()).await;
        assert!(result.is_ok());
        assert_eq!(primary.count(), 1, "primary should be called once");
        assert_eq!(secondary.count(), 0, "secondary should not be called");
    }

    #[tokio::test]
    async fn falls_back_to_secondary_on_primary_failure() {
        let primary = MockTransport::new(false);
        let secondary = MockTransport::new(true);

        let mut service = SequentialFallbackLayer.layer(vec![primary.clone(), secondary.clone()]);

        let result = service.call(dummy_request()).await;
        assert!(result.is_ok());
        assert_eq!(primary.count(), 1, "primary should be attempted");
        assert_eq!(secondary.count(), 1, "secondary should be used as fallback");
    }

    #[tokio::test]
    async fn returns_error_when_all_transports_fail() {
        let primary = MockTransport::new(false);
        let secondary = MockTransport::new(false);

        let mut service = SequentialFallbackLayer.layer(vec![primary.clone(), secondary.clone()]);

        let result = service.call(dummy_request()).await;
        assert!(result.is_err());
        assert_eq!(primary.count(), 1);
        assert_eq!(secondary.count(), 1);
    }

    #[tokio::test]
    async fn skips_transport_after_max_consecutive_failures() {
        let primary = MockTransport::new(false);
        let secondary = MockTransport::new(true);

        let mut service = SequentialFallbackLayer.layer(vec![primary.clone(), secondary.clone()]);

        // Trigger MAX_CONSECUTIVE_FAILURES (3) failures (request_num 0..2)
        for _ in 0..MAX_CONSECUTIVE_FAILURES {
            let _ = service.call(dummy_request()).await;
        }

        let primary_count_before = primary.count();
        let secondary_count_before = secondary.count();

        // request_num 3: 3 % 10 != 0, so primary should be skipped
        let result = service.call(dummy_request()).await;
        assert!(result.is_ok());
        assert_eq!(primary.count(), primary_count_before, "primary should be skipped");
        assert_eq!(
            secondary.count(),
            secondary_count_before + 1,
            "secondary should handle the request directly"
        );
    }

    #[tokio::test]
    async fn retries_skipped_transport_after_retry_interval() {
        let primary = MockTransport::new(false);
        let secondary = MockTransport::new(true);

        let mut service = SequentialFallbackLayer.layer(vec![primary.clone(), secondary.clone()]);

        // Exhaust primary: request_num 0..2 → primary is skipped after request 2
        for _ in 0..MAX_CONSECUTIVE_FAILURES {
            let _ = service.call(dummy_request()).await;
        }

        // Make primary succeed so we can detect when it gets retried
        primary.set_succeed(true);

        // Requests 3..9 (7 requests): primary is still skipped (N % 10 != 0)
        let skip_requests = RETRY_INTERVAL - MAX_CONSECUTIVE_FAILURES as u64;
        for _ in 0..skip_requests {
            let _ = service.call(dummy_request()).await;
        }

        let primary_count_before = primary.count();

        // request_num 10: 10 % 10 == 0 → primary is retried
        let result = service.call(dummy_request()).await;
        assert!(result.is_ok());
        assert_eq!(
            primary.count(),
            primary_count_before + 1,
            "primary should be retried at RETRY_INTERVAL"
        );
    }

    #[tokio::test]
    async fn primary_always_tried_when_all_skipped() {
        let primary = MockTransport::new(false);
        let secondary = MockTransport::new(false);

        let mut service = SequentialFallbackLayer.layer(vec![primary.clone(), secondary.clone()]);

        // Exhaust both transports so both are skipped
        for _ in 0..MAX_CONSECUTIVE_FAILURES {
            let _ = service.call(dummy_request()).await;
        }

        let primary_count_before = primary.count();

        // request_num 3: 3 % 10 != 0 → all transports are skipped.
        // Primary should still be attempted as a last resort (not silently dropped).
        let result = service.call(dummy_request()).await;
        assert!(result.is_err(), "all transports fail so result should be Err");
        assert_eq!(
            primary.count(),
            primary_count_before + 1,
            "primary should always be tried even when skipped"
        );
    }

    // ── Integration test: retry wraps sequential fallback ────────────

    /// A transport that fails for the first `fail_count` calls then succeeds,
    /// appending its name to a shared call log on every call.
    #[derive(Clone)]
    struct FailNTransport {
        name: &'static str,
        call_count: Arc<AtomicU32>,
        fail_count: u32,
        call_log: Arc<Mutex<Vec<&'static str>>>,
    }

    impl FailNTransport {
        fn new(
            name: &'static str,
            fail_count: u32,
            call_log: Arc<Mutex<Vec<&'static str>>>,
        ) -> Self {
            Self { name, call_count: Arc::new(AtomicU32::new(0)), fail_count, call_log }
        }

        fn count(&self) -> u32 {
            self.call_count.load(Ordering::SeqCst)
        }
    }

    impl Service<RequestPacket> for FailNTransport {
        type Response = ResponsePacket;
        type Error = TransportError;
        type Future = TransportFut<'static>;

        fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
            Poll::Ready(Ok(()))
        }

        fn call(&mut self, _req: RequestPacket) -> Self::Future {
            let n = self.call_count.fetch_add(1, Ordering::SeqCst);
            let should_fail = n < self.fail_count;
            let name = self.name;
            let call_log = self.call_log.clone();
            Box::pin(async move {
                call_log.lock().unwrap().push(name);
                if should_fail {
                    Err(TransportErrorKind::custom_str("mock failure"))
                } else {
                    Ok(mock_response())
                }
            })
        }
    }

    /// A retry policy that always retries with zero backoff, for test use only.
    #[derive(Debug, Clone)]
    struct AlwaysRetryPolicy;

    impl RetryPolicy for AlwaysRetryPolicy {
        fn should_retry(&self, _error: &TransportError) -> bool {
            true
        }

        fn backoff_hint(&self, _error: &TransportError) -> Option<std::time::Duration> {
            Some(std::time::Duration::ZERO)
        }
    }

    /// Verify that the retry layer wraps the sequential fallback, not the other way around.
    ///
    /// With the correct ordering (retry → fallback → transports):
    /// - Round 1: primary fails → secondary fails → all URLs exhausted → retry triggers
    /// - Round 2: primary fails → secondary succeeds → returns Ok
    ///
    /// Call log: ["primary", "secondary", "primary", "secondary"]
    ///
    /// Under the old (incorrect) ordering where retry wrapped each transport individually, the
    /// primary would be retried multiple times before the fallback tried the secondary, producing
    /// a log like ["primary", "primary", ..., "secondary"].
    #[tokio::test]
    async fn retry_layer_retries_after_all_fallbacks_exhausted() {
        let call_log: Arc<Mutex<Vec<&'static str>>> = Arc::new(Mutex::new(Vec::new()));
        // Primary always fails; secondary fails once then succeeds.
        let primary = FailNTransport::new("primary", u32::MAX, call_log.clone());
        let secondary = FailNTransport::new("secondary", 1, call_log.clone());

        let retry_layer = RetryBackoffLayer::new_with_policy(3, 0, u64::MAX, AlwaysRetryPolicy);

        let mut service = ServiceBuilder::new()
            .layer(retry_layer)
            .layer(SequentialFallbackLayer)
            .service(vec![primary.clone(), secondary.clone()]);

        let result = service.call(dummy_request()).await;

        assert!(result.is_ok(), "should succeed on the second retry round");
        assert_eq!(primary.count(), 2, "primary tried once per retry round");
        assert_eq!(secondary.count(), 2, "secondary tried once per retry round");
        assert_eq!(
            *call_log.lock().unwrap(),
            vec!["primary", "secondary", "primary", "secondary"],
            "fallback (primary→secondary) must complete before retry triggers a new round"
        );
    }
}
