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

/// Number of consecutive failures before a transport is skipped. This number is fairly low
/// as each transport is already wrapped within a retry layer, and we want to skip unhealthy transports quickly.
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
/// If a transport fails [`MAX_CONSECUTIVE_FAILURES`] times in a row it is
/// temporarily skipped. Skipped transports are retried every [`RETRY_INTERVAL`]
/// requests to detect recovery.
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

        for tracked in self.transports.iter() {
            if !tracked.should_try(request_num) {
                tracing::trace!(transport_id = tracked.id, "Skipping transport (unhealthy)");
                continue;
            }

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
    use std::sync::atomic::{AtomicBool, AtomicU32};
    use tower::Layer;

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

        // Trigger MAX_CONSECUTIVE_FAILURES failures (request_num 0..19)
        for _ in 0..MAX_CONSECUTIVE_FAILURES {
            let _ = service.call(dummy_request()).await;
        }

        let primary_count_before = primary.count();
        let secondary_count_before = secondary.count();

        // request_num 20: 20 % 100 != 0, so primary should be skipped
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

        // Exhaust primary: request_num 0..19 → primary is skipped after request 19
        for _ in 0..MAX_CONSECUTIVE_FAILURES {
            let _ = service.call(dummy_request()).await;
        }

        // Make primary succeed so we can detect when it gets retried
        primary.set_succeed(true);

        // Requests 20..99 (80 requests): primary is still skipped (N % 100 != 0)
        let skip_requests = RETRY_INTERVAL - MAX_CONSECUTIVE_FAILURES as u64;
        for _ in 0..skip_requests {
            let _ = service.call(dummy_request()).await;
        }

        let primary_count_before = primary.count();

        // request_num 100: 100 % 100 == 0 → primary is retried
        let result = service.call(dummy_request()).await;
        assert!(result.is_ok());
        assert_eq!(
            primary.count(),
            primary_count_before + 1,
            "primary should be retried at RETRY_INTERVAL"
        );
    }
}
