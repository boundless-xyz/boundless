// Copyright (c) 2025 RISC Zero, Inc.
//
// All rights reserved.

use anyhow::Result;
use std::process::Stdio;
use tokio::io::{self, AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::task;
use tokio::time::{sleep, Duration};

const TOXIPROXY_URL: &str = "http://localhost:8474";

pub async fn up() -> Result<()> {
    // Check if toxiproxy is already running
    if let Ok(output) = Command::new("pgrep").arg("toxiproxy-server").output().await {
        if !output.stdout.is_empty() {
            tracing::info!("Toxiproxy is already running.");
            return Ok(());
        }
    }

    // Try to start toxiproxy
    let mut child = Command::new("toxiproxy-server")
        .args(["-port", "8474"])
        .stdout(Stdio::piped())
        .spawn()
        .map_err(|e| {
            if e.kind() == io::ErrorKind::NotFound {
                io::Error::new(io::ErrorKind::Other, "Toxiproxy is not installed or not in the PATH. See https://github.com/Shopify/toxiproxy")
            } else {
                e
            }
        })?;

    // Capture and log the output of the Toxiproxy process
    let stdout = child.stdout.take().expect("Failed to capture stdout");
    task::spawn(async move {
        let reader = BufReader::new(stdout);
        let mut lines = reader.lines();
        while let Ok(Some(line)) = lines.next_line().await {
            tracing::trace!("Toxiproxy: {}", line);
        }
    });

    // Wait for the HTTP API to come up
    let client = reqwest::Client::new();
    let start = tokio::time::Instant::now();
    let timeout = Duration::from_secs(10);
    loop {
        match client.get(format!("{}/proxies", TOXIPROXY_URL)).send().await {
            Ok(response) => {
                if response.status().is_success() {
                    tracing::info!("HTTP API is up.");
                    break;
                } else {
                    tracing::info!("HTTP API not ready, status: {}", response.status());
                }
            }
            Err(err) => {
                tracing::info!("HTTP API not ready, err: {}", err);
            }
        }

        if start.elapsed() > timeout {
            return Err(anyhow::anyhow!("Timeout waiting for Toxiproxy to start"));
        }

        // Sleep for a short duration before retrying
        sleep(Duration::from_millis(100)).await;
    }

    tracing::info!("Toxiproxy started successfully.");
    Ok(())
}

pub async fn down() -> Result<()> {
    Command::new("pkill").arg("toxiproxy-server").output().await?;
    tracing::info!("Toxiproxy killed");
    Ok(())
}

pub async fn proxy_rpc(rpc_url: &str, rng_seed: u64) -> Result<String> {
    let client = reqwest::Client::new();
    let listen_addr = "localhost:22220";
    let proxy_config = serde_json::json!({
        "name": "anvil_proxy",
        "listen": listen_addr,
        "upstream":rpc_url.strip_prefix("http://").unwrap(),
        "enabled": true,
        "rand_seed": rng_seed,
    });
    // remove proxy if it already exists
    client.delete(format!("{}/proxies/anvil_proxy", TOXIPROXY_URL)).send().await?;
    client.post(format!("{}/proxies", TOXIPROXY_URL)).json(&proxy_config).send().await?;
    tracing::info!("Started proxy listening on {} forwarding to {}", listen_addr, rpc_url);

    Ok(format!("http://{}", listen_addr))
}

pub async fn add_reset_toxic(toxicity: f32) -> Result<()> {
    let client = reqwest::Client::new();
    let toxic_config = serde_json::json!({
        "name": "anvil_proxy_reset",
        "type": "reset_peer",
        "toxicity": toxicity,
        "stream": "downstream",
    });
    client
        .post(format!("{}/proxies/anvil_proxy/toxics", TOXIPROXY_URL))
        .json(&toxic_config)
        .send()
        .await?
        .error_for_status()?;
    tracing::info!("Added reset_peer toxic to anvil_proxy with toxicity {}", toxicity);
    Ok(())
}
