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

use std::task::{Context, Poll};

use alloy::rpc::json_rpc::{RequestPacket, ResponsePacket};
use alloy::transports::{TransportError, TransportFut};
use tower::{Layer, Service};

// ── Layer ───────────────────────────────────────────────────────

#[derive(Clone, Default)]
pub struct RpcMetricsLayer;

impl RpcMetricsLayer {
    pub fn new() -> Self {
        Self
    }
}

impl<S> Layer<S> for RpcMetricsLayer {
    type Service = RpcMetrics<S>;

    fn layer(&self, inner: S) -> Self::Service {
        RpcMetrics { inner }
    }
}

// ── Service ─────────────────────────────────────────────────────

#[derive(Clone)]
pub struct RpcMetrics<S> {
    inner: S,
}

impl<S> Service<RequestPacket> for RpcMetrics<S>
where
    S: Service<RequestPacket, Response = ResponsePacket, Error = TransportError>
        + Send
        + Clone
        + 'static,
    S::Future: Send,
{
    type Response = ResponsePacket;
    type Error = TransportError;
    type Future = TransportFut<'static>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, req: RequestPacket) -> Self::Future {
        if !tracing::enabled!(tracing::Level::DEBUG) {
            let mut inner = self.inner.clone();
            return Box::pin(async move { inner.call(req).await });
        }

        let (methods, req_bytes) = match &req {
            RequestPacket::Single(r) => {
                let serialized = serde_json::to_string(r).unwrap_or_default();
                (vec![r.method().to_string()], serialized.len() as u64)
            }
            RequestPacket::Batch(batch) => {
                let serialized = serde_json::to_string(batch).unwrap_or_default();
                let methods: Vec<String> = batch.iter().map(|r| r.method().to_string()).collect();
                (methods, serialized.len() as u64)
            }
        };

        let start = std::time::Instant::now();
        let fut = self.inner.call(req);

        Box::pin(async move {
            let result = fut.await;
            let elapsed_ms = start.elapsed().as_millis();

            let (resp_bytes, rpc_error_code, error) = match &result {
                Ok(resp) => {
                    let bytes = match resp {
                        ResponsePacket::Single(r) => {
                            serde_json::to_string(r).unwrap_or_default().len() as u64
                        }
                        ResponsePacket::Batch(batch) => {
                            serde_json::to_string(batch).unwrap_or_default().len() as u64
                        }
                    };
                    let error_code = resp.as_error().map(|e| e.code);
                    (bytes, error_code, None)
                }
                Err(e) => (0, None, Some(e.to_string())),
            };

            let method_str = methods.join(",");
            tracing::debug!(
                method = method_str,
                req_bytes,
                resp_bytes,
                duration_ms = elapsed_ms as u64,
                ok = result.is_ok() && rpc_error_code.is_none(),
                rpc_error_code = rpc_error_code.map(|c| c.to_string()),
                error,
                "rpc call"
            );

            result
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::rpc::json_rpc::{Id, Request};
    use alloy::transports::TransportErrorKind;
    use std::sync::{
        atomic::{AtomicBool, AtomicU32, Ordering},
        Arc,
    };
    use std::task::{Context, Poll};
    use tower::Layer;
    use tracing_test::traced_test;

    fn dummy_request() -> RequestPacket {
        let req: Request<()> = Request::new("eth_blockNumber", Id::Number(1), ());
        RequestPacket::Single(req.serialize().unwrap())
    }

    fn mock_response() -> ResponsePacket {
        serde_json::from_str(r#"{"jsonrpc":"2.0","id":1,"result":"0x1"}"#).unwrap()
    }

    fn mock_rpc_error_response() -> ResponsePacket {
        serde_json::from_str(
            r#"{"jsonrpc":"2.0","id":1,"error":{"code":-32000,"message":"execution reverted"}}"#,
        )
        .unwrap()
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

    #[derive(Clone)]
    struct RpcErrorTransport;

    impl Service<RequestPacket> for RpcErrorTransport {
        type Response = ResponsePacket;
        type Error = TransportError;
        type Future = TransportFut<'static>;

        fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
            Poll::Ready(Ok(()))
        }

        fn call(&mut self, _req: RequestPacket) -> Self::Future {
            Box::pin(async move { Ok(mock_rpc_error_response()) })
        }
    }

    #[tokio::test]
    async fn success_passes_through() {
        let mock = MockTransport::new(true);
        let mut svc = RpcMetricsLayer::new().layer(mock.clone());
        let result = svc.call(dummy_request()).await;
        assert!(result.is_ok());
        assert_eq!(mock.count(), 1);
    }

    #[tokio::test]
    async fn error_passes_through() {
        let mock = MockTransport::new(false);
        let mut svc = RpcMetricsLayer::new().layer(mock.clone());
        let result = svc.call(dummy_request()).await;
        assert!(result.is_err());
        assert_eq!(mock.count(), 1);
    }

    #[traced_test]
    #[tokio::test]
    async fn logs_on_success() {
        let mock = MockTransport::new(true);
        let mut svc = RpcMetricsLayer::new().layer(mock);
        svc.call(dummy_request()).await.unwrap();
        assert!(logs_contain("rpc call"));
    }

    #[traced_test]
    #[tokio::test]
    async fn logs_on_error() {
        let mock = MockTransport::new(false);
        let mut svc = RpcMetricsLayer::new().layer(mock);
        let _ = svc.call(dummy_request()).await;
        assert!(logs_contain("rpc call"));
        assert!(logs_contain("mock failure"));
    }

    #[traced_test]
    #[tokio::test]
    async fn logs_rpc_error_code() {
        let mut svc = RpcMetricsLayer::new().layer(RpcErrorTransport);
        let result = svc.call(dummy_request()).await;
        assert!(result.is_ok(), "transport succeeded");
        assert!(logs_contain("-32000"), "error code should be logged");
    }
}
