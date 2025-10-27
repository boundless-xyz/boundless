// Copyright 2025 RISC Zero, Inc.
//
// Use of this source code is governed by the Business Source License
// as found in the LICENSE-BSL file.

use anyhow::Result;
use prometheus::{Encoder, TextEncoder};

/// Start the metrics server to expose Prometheus metrics
pub async fn start_metrics_server(port: u16) -> Result<()> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    let addr = format!("0.0.0.0:{}", port);
    let listener = TcpListener::bind(&addr).await?;
    tracing::info!("Metrics server listening on http://{}", addr);

    loop {
        match listener.accept().await {
            Ok((mut stream, _)) => {
                tokio::task::spawn(async move {
                    let mut buffer = Vec::new();
                    if let Err(e) = stream.read_buf(&mut buffer).await {
                        tracing::error!("Failed to read request: {}", e);
                        return;
                    }

                    let request = String::from_utf8_lossy(&buffer);

                    // Parse request to check if it's for /metrics
                    if !request.trim().starts_with("GET /metrics") {
                        let _ = stream.write_all(b"HTTP/1.1 404 Not Found\r\n\r\n").await;
                        return;
                    }

                    // Gather metrics using the default registry
                    let encoder = TextEncoder::new();
                    let mut metric_families = vec![];

                    // Try to gather from default registry
                    let gather_result = prometheus::gather();
                    tracing::info!(
                        "prometheus::gather() returned {} families",
                        gather_result.len()
                    );
                    metric_families.extend(gather_result);

                    let mut metrics_buffer = Vec::new();

                    if let Err(e) = encoder.encode(&metric_families, &mut metrics_buffer) {
                        tracing::error!("Failed to encode metrics: {}", e);
                        let _ =
                            stream.write_all(b"HTTP/1.1 500 Internal Server Error\r\n\r\n").await;
                        return;
                    }

                    let response = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: {}\r\nContent-Length: {}\r\n\r\n",
                        encoder.format_type(),
                        metrics_buffer.len()
                    );

                    if let Err(e) = stream.write_all(response.as_bytes()).await {
                        tracing::error!("Failed to write response headers: {}", e);
                        return;
                    }

                    if let Err(e) = stream.write_all(&metrics_buffer).await {
                        tracing::error!("Failed to write metrics: {}", e);
                    }

                    tracing::debug!("Served {} bytes of metrics data", metrics_buffer.len());
                });
            }
            Err(e) => {
                tracing::error!("Failed to accept connection: {}", e);
            }
        }
    }
}
