use alloy::rpc::json_rpc::{RequestPacket, ResponsePacket};
use alloy::transports::{TransportError, TransportFut};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::task::{Context, Poll};
use tower::{Layer, Service};


fn now_iso() -> String {
    chrono::Utc::now().format("%Y-%m-%dT%H:%M:%S%.6fZ").to_string()
}

// ── Stats ───────────────────────────────────────────────────────

#[derive(Default, Debug)]
pub struct RpcMetricsStats {
    pub total_requests: AtomicU64,
    pub total_req_bytes: AtomicU64,
    pub total_resp_bytes: AtomicU64,
}

impl RpcMetricsStats {
    pub fn summary(&self) {
        let reqs = self.total_requests.load(Ordering::Relaxed);
        let sent = self.total_req_bytes.load(Ordering::Relaxed);
        let recv = self.total_resp_bytes.load(Ordering::Relaxed);
        eprintln!(
            "{} RPC_SUMMARY total_requests={reqs} total_req_bytes={sent} total_resp_bytes={recv} total_bytes={}",
            now_iso(),
            sent + recv
        );
    }
}

// ── Layer ───────────────────────────────────────────────────────

#[derive(Clone)]
pub struct RpcMetricsLayer {
    stats: Arc<RpcMetricsStats>,
}

impl RpcMetricsLayer {
    pub fn new() -> Self {
        Self {
            stats: Arc::new(RpcMetricsStats::default()),
        }
    }

    pub fn stats(&self) -> Arc<RpcMetricsStats> {
        self.stats.clone()
    }
}

impl Default for RpcMetricsLayer {
    fn default() -> Self {
        Self::new()
    }
}

impl<S> Layer<S> for RpcMetricsLayer {
    type Service = RpcMetrics<S>;

    fn layer(&self, inner: S) -> Self::Service {
        RpcMetrics {
            inner,
            stats: self.stats.clone(),
        }
    }
}

// ── Service ─────────────────────────────────────────────────────

#[derive(Clone)]
pub struct RpcMetrics<S> {
    inner: S,
    stats: Arc<RpcMetricsStats>,
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

        let stats = self.stats.clone();
        stats
            .total_requests
            .fetch_add(methods.len() as u64, Ordering::Relaxed);
        stats.total_req_bytes.fetch_add(req_bytes, Ordering::Relaxed);

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

            stats
                .total_resp_bytes
                .fetch_add(resp_bytes, Ordering::Relaxed);

            let method_str = methods.join(",");
            eprintln!(
                "{} RPC_METRICS method={method_str} req_bytes={req_bytes} resp_bytes={resp_bytes} duration_ms={elapsed_ms}",
                now_iso()
            );

            Ok(resp)
        })
    }
}