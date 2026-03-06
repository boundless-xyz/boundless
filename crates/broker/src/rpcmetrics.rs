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
            let resp = fut.await?;
            let elapsed_ms = start.elapsed().as_millis();

            let resp_bytes = match &resp {
                ResponsePacket::Single(r) => serde_json::to_string(r).unwrap_or_default().len(),
                ResponsePacket::Batch(batch) => {
                    serde_json::to_string(batch).unwrap_or_default().len()
                }
            } as u64;

            let method_str = methods.join(",");
            tracing::debug!(
                method = method_str,
                req_bytes,
                resp_bytes,
                duration_ms = elapsed_ms as u64,
                "rpc call"
            );

            Ok(resp)
        })
    }
}
