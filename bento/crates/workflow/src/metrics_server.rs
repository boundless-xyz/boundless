// Copyright 2025 RISC Zero, Inc.
//
// Use of this source code is governed by the Business Source License
// as found in the LICENSE-BSL file.

use anyhow::Result;
use prometheus::{Encoder, TextEncoder};

/// Start the metrics server to expose Prometheus metrics
pub async fn start_metrics_server(port: u16) -> Result<()> {
    use std::io::{Read, Write};
    use std::net::TcpListener;

    let addr = format!("0.0.0.0:{}", port);
    let listener = TcpListener::bind(&addr)?;
    tracing::info!("Metrics server listening on http://{}", addr);

    loop {
        match listener.accept() {
            Ok((mut stream, _)) => {
                tokio::task::spawn(async move {
                    let mut buffer = String::new();
                    if let Err(e) = stream.read_to_string(&mut buffer) {
                        tracing::error!("Failed to read request: {}", e);
                        return;
                    }

                    // Parse request to check if it's for /metrics
                    if !buffer.trim().starts_with("GET /metrics") {
                        let _ = stream.write_all(b"HTTP/1.1 404 Not Found\r\n\r\n");
                        return;
                    }

                    // Gather metrics
                    let encoder = TextEncoder::new();
                    let metric_families = prometheus::gather();
                    let mut buffer = Vec::new();

                    if let Err(e) = encoder.encode(&metric_families, &mut buffer) {
                        tracing::error!("Failed to encode metrics: {}", e);
                        let _ = stream.write_all(b"HTTP/1.1 500 Internal Server Error\r\n\r\n");
                        return;
                    }

                    let response = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: {}\r\nContent-Length: {}\r\n\r\n",
                        encoder.format_type(),
                        buffer.len()
                    );

                    if let Err(e) = stream.write_all(response.as_bytes()) {
                        tracing::error!("Failed to write response headers: {}", e);
                        return;
                    }

                    if let Err(e) = stream.write_all(&buffer) {
                        tracing::error!("Failed to write metrics: {}", e);
                    }
                });
            }
            Err(e) => {
                tracing::error!("Failed to accept connection: {}", e);
            }
        }
    }
}
