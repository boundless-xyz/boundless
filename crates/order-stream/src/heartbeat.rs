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

use std::sync::Arc;
use std::time::Duration;

use crate::{AppError, AppState};
use alloy::primitives::{Address, Signature as PrimSignature, U256};
use aws_sdk_kinesis::primitives::Blob;
use axum::{
    body::Bytes,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use boundless_market::{
    contracts::IBoundlessMarket,
    telemetry::{BrokerHeartbeat, RequestHeartbeat},
};
use moka::future::Cache;

const BALANCE_CACHE_TTL_SECS: u64 = 1800;
const BALANCE_CACHE_MAX_ENTRIES: u64 = 10_000;

/// Cached collateral balance keyed by address.
pub(crate) type BalanceCache = Cache<Address, U256>;

/// Creates a new balance cache with TTL eviction.
pub(crate) fn new_balance_cache() -> BalanceCache {
    Cache::builder()
        .max_capacity(BALANCE_CACHE_MAX_ENTRIES)
        .time_to_live(Duration::from_secs(BALANCE_CACHE_TTL_SECS))
        .build()
}

// TelemetryForwarder trait + implementations
#[async_trait::async_trait]
pub(crate) trait TelemetryForwarder: Send + Sync {
    /// Forward a single heartbeat payload.
    async fn forward_heartbeat(&self, partition_key: &str, data: &[u8]) -> anyhow::Result<()>;

    /// Forward a batch of request evaluation records.
    async fn forward_evaluation_records(
        &self,
        partition_key: &str,
        records: Vec<Vec<u8>>,
    ) -> anyhow::Result<()>;

    /// Forward a batch of request completion records.
    async fn forward_completion_records(
        &self,
        partition_key: &str,
        records: Vec<Vec<u8>>,
    ) -> anyhow::Result<()>;
}

/// Forwards telemetry records to AWS Kinesis Data Streams.
pub(crate) struct KinesisForwarder {
    client: aws_sdk_kinesis::Client,
    heartbeat_stream: String,
    evaluations_stream: String,
    completions_stream: String,
}

impl KinesisForwarder {
    pub(crate) fn new(
        client: aws_sdk_kinesis::Client,
        heartbeat_stream: String,
        evaluations_stream: String,
        completions_stream: String,
    ) -> Self {
        Self { client, heartbeat_stream, evaluations_stream, completions_stream }
    }

    async fn put_records_to_stream(
        &self,
        stream: &str,
        partition_key: &str,
        records: Vec<Vec<u8>>,
    ) -> anyhow::Result<()> {
        if records.is_empty() {
            return Ok(());
        }

        let entries: Vec<_> = records
            .into_iter()
            .map(|data| {
                aws_sdk_kinesis::types::PutRecordsRequestEntry::builder()
                    .partition_key(partition_key)
                    .data(Blob::new(data))
                    .build()
                    .expect("valid entry")
            })
            .collect();

        self.client
            .put_records()
            .stream_name(stream)
            .set_records(Some(entries))
            .send()
            .await
            .map_err(|e| anyhow::anyhow!("Kinesis put_records failed: {e}"))?;

        Ok(())
    }
}

#[async_trait::async_trait]
impl TelemetryForwarder for KinesisForwarder {
    async fn forward_heartbeat(&self, partition_key: &str, data: &[u8]) -> anyhow::Result<()> {
        self.client
            .put_record()
            .stream_name(&self.heartbeat_stream)
            .partition_key(partition_key)
            .data(Blob::new(data.to_vec()))
            .send()
            .await
            .map_err(|e| anyhow::anyhow!("Kinesis put_record failed: {e}"))?;
        Ok(())
    }

    async fn forward_evaluation_records(
        &self,
        partition_key: &str,
        records: Vec<Vec<u8>>,
    ) -> anyhow::Result<()> {
        self.put_records_to_stream(&self.evaluations_stream, partition_key, records).await
    }

    async fn forward_completion_records(
        &self,
        partition_key: &str,
        records: Vec<Vec<u8>>,
    ) -> anyhow::Result<()> {
        self.put_records_to_stream(&self.completions_stream, partition_key, records).await
    }
}

/// No-op forwarder used when Kinesis is not configured. Logs payloads at debug level.
pub(crate) struct NoopForwarder;

#[async_trait::async_trait]
impl TelemetryForwarder for NoopForwarder {
    async fn forward_heartbeat(&self, partition_key: &str, _data: &[u8]) -> anyhow::Result<()> {
        tracing::debug!("Kinesis not configured, dropping heartbeat for {partition_key}");
        Ok(())
    }

    async fn forward_evaluation_records(
        &self,
        partition_key: &str,
        records: Vec<Vec<u8>>,
    ) -> anyhow::Result<()> {
        tracing::debug!(
            "Kinesis not configured, dropping {} evaluation records for {partition_key}",
            records.len()
        );
        Ok(())
    }

    async fn forward_completion_records(
        &self,
        partition_key: &str,
        records: Vec<Vec<u8>>,
    ) -> anyhow::Result<()> {
        tracing::debug!(
            "Kinesis not configured, dropping {} completion records for {partition_key}",
            records.len()
        );
        Ok(())
    }
}

/// Recover the signer address from an EIP-191 personal_sign signature over the raw body bytes.
fn recover_signer(body: &[u8], sig_hex: &str) -> Result<Address, AppError> {
    let sig_bytes = hex::decode(sig_hex.strip_prefix("0x").unwrap_or(sig_hex))
        .map_err(|_| AppError::QueryParamErr("X-Signature: invalid hex"))?;

    if sig_bytes.len() != 65 {
        return Err(AppError::QueryParamErr("X-Signature: must be 65 bytes"));
    }

    let v = sig_bytes[64];
    let parity = if v >= 27 { v - 27 } else { v } != 0;
    let signature = PrimSignature::from_bytes_and_parity(&sig_bytes[..64], parity);

    let hash = alloy::primitives::eip191_hash_message(body);
    let recovered = signature
        .recover_address_from_prehash(&hash)
        .map_err(|_| AppError::QueryParamErr("X-Signature: recovery failed"))?;

    Ok(recovered)
}

/// Verifies the request signature and checks the broker's collateral balance.
async fn verify_heartbeat_auth(
    state: &AppState,
    headers: &HeaderMap,
    body: &[u8],
    claimed_address: Address,
) -> Result<(), impl IntoResponse> {
    let sig_header = headers.get("X-Signature").and_then(|v| v.to_str().ok()).ok_or_else(|| {
        (StatusCode::BAD_REQUEST, "Missing X-Signature header".to_string()).into_response()
    })?;

    let recovered = recover_signer(body, sig_header)
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("Signature error: {e}")).into_response())?;

    if recovered != claimed_address {
        return Err((
            StatusCode::UNAUTHORIZED,
            format!("Signer {recovered} does not match claimed address {claimed_address}"),
        )
            .into_response());
    }

    if !state.config.bypass_addrs.contains(&claimed_address) {
        let balance = if let Some(cached) = state.balance_cache.get(&claimed_address).await {
            cached
        } else {
            let market =
                IBoundlessMarket::new(state.config.market_address, state.rpc_provider.clone());
            let balance =
                market.balanceOfCollateral(claimed_address).call().await.map_err(|e| {
                    tracing::warn!("Failed to check balance for {claimed_address}: {e}");
                    (StatusCode::INTERNAL_SERVER_ERROR, "Failed to check balance".to_string())
                        .into_response()
                })?;
            state.balance_cache.insert(claimed_address, balance).await;
            balance
        };

        if balance < state.config.min_balance {
            return Err((
                StatusCode::UNAUTHORIZED,
                format!(
                    "Insufficient collateral balance: {} < {}",
                    balance, state.config.min_balance
                ),
            )
                .into_response());
        }
    }

    Ok(())
}

/// POST /api/v2/heartbeats
///
/// Accepts a signed broker identity heartbeat and forwards it to the telemetry backend.
pub(crate) async fn submit_heartbeat(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<StatusCode, Response> {
    let heartbeat: BrokerHeartbeat = serde_json::from_slice(&body).map_err(|e| {
        (StatusCode::BAD_REQUEST, format!("Invalid heartbeat payload: {e}")).into_response()
    })?;

    verify_heartbeat_auth(&state, &headers, &body, heartbeat.broker_address)
        .await
        .map_err(|e| e.into_response())?;

    let partition_key = format!("{}", heartbeat.broker_address);
    state.telemetry.forward_heartbeat(&partition_key, &body).await.map_err(|e| {
        tracing::error!("Failed to forward heartbeat: {e}");
        (StatusCode::SERVICE_UNAVAILABLE, "Failed to forward heartbeat".to_string()).into_response()
    })?;

    tracing::info!(
        broker = %heartbeat.broker_address,
        version = %heartbeat.version,
        uptime_secs = heartbeat.uptime_secs,
        committed_orders = heartbeat.committed_orders_count,
        pending_preflight = heartbeat.pending_preflight_count,
        "Heartbeat accepted"
    );

    Ok(StatusCode::ACCEPTED)
}

/// POST /api/v2/heartbeats/requests
///
/// Accepts a signed request evaluation/completion heartbeat and forwards records to the
/// telemetry backend. Evaluations and completions are sent to separate Kinesis streams.
pub(crate) async fn submit_request_heartbeat(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<StatusCode, Response> {
    let heartbeat: RequestHeartbeat = serde_json::from_slice(&body).map_err(|e| {
        (StatusCode::BAD_REQUEST, format!("Invalid request heartbeat payload: {e}")).into_response()
    })?;

    verify_heartbeat_auth(&state, &headers, &body, heartbeat.broker_address)
        .await
        .map_err(|e| e.into_response())?;

    let partition_key = format!("{}", heartbeat.broker_address);

    let eval_count = heartbeat.evaluated.len();
    let comp_count = heartbeat.completed.len();

    let eval_records: Vec<Vec<u8>> =
        heartbeat.evaluated.iter().filter_map(|eval| serde_json::to_vec(eval).ok()).collect();

    let comp_records: Vec<Vec<u8>> =
        heartbeat.completed.iter().filter_map(|comp| serde_json::to_vec(comp).ok()).collect();

    state.telemetry.forward_evaluation_records(&partition_key, eval_records).await.map_err(
        |e| {
            tracing::error!("Failed to forward evaluation records: {e}");
            (StatusCode::SERVICE_UNAVAILABLE, "Failed to forward evaluation records".to_string())
                .into_response()
        },
    )?;

    state.telemetry.forward_completion_records(&partition_key, comp_records).await.map_err(
        |e| {
            tracing::error!("Failed to forward completion records: {e}");
            (StatusCode::SERVICE_UNAVAILABLE, "Failed to forward completion records".to_string())
                .into_response()
        },
    )?;

    tracing::info!(
        broker = %heartbeat.broker_address,
        evaluations = eval_count,
        completions = comp_count,
        events_dropped = heartbeat.events_dropped,
        "Request heartbeat accepted"
    );

    Ok(StatusCode::ACCEPTED)
}

#[cfg(test)]
pub(crate) mod mock {
    use super::*;
    use std::sync::Mutex;

    #[derive(Debug, Clone)]
    #[allow(dead_code)]
    pub(crate) enum CapturedRecord {
        Heartbeat { partition_key: String, data: Vec<u8> },
        EvaluationBatch { partition_key: String, records: Vec<Vec<u8>> },
        CompletionBatch { partition_key: String, records: Vec<Vec<u8>> },
    }

    pub(crate) struct MockForwarder {
        pub(crate) captured: Mutex<Vec<CapturedRecord>>,
    }

    #[allow(dead_code)]
    impl MockForwarder {
        pub(crate) fn new() -> Self {
            Self { captured: Mutex::new(Vec::new()) }
        }
    }

    #[async_trait::async_trait]
    impl TelemetryForwarder for MockForwarder {
        async fn forward_heartbeat(&self, partition_key: &str, data: &[u8]) -> anyhow::Result<()> {
            self.captured.lock().unwrap().push(CapturedRecord::Heartbeat {
                partition_key: partition_key.to_string(),
                data: data.to_vec(),
            });
            Ok(())
        }

        async fn forward_evaluation_records(
            &self,
            partition_key: &str,
            records: Vec<Vec<u8>>,
        ) -> anyhow::Result<()> {
            self.captured.lock().unwrap().push(CapturedRecord::EvaluationBatch {
                partition_key: partition_key.to_string(),
                records,
            });
            Ok(())
        }

        async fn forward_completion_records(
            &self,
            partition_key: &str,
            records: Vec<Vec<u8>>,
        ) -> anyhow::Result<()> {
            self.captured.lock().unwrap().push(CapturedRecord::CompletionBatch {
                partition_key: partition_key.to_string(),
                records,
            });
            Ok(())
        }
    }
}
