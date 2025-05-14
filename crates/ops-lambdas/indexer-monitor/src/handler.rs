// Copyright (c) 2025 RISC Zero, Inc.
//
// All rights reserved.

use std::str::FromStr;
use std::time::Duration;

use alloy::primitives::Address;
use anyhow::{Context, Result};
use aws_config::retry::{RetryConfigBuilder, RetryMode};
use aws_config::Region;
use aws_sdk_cloudwatch::types::{MetricDatum, StandardUnit};
use aws_sdk_cloudwatch::Client as CloudWatchClient;
use aws_smithy_types::retry::ReconnectMode;
use aws_smithy_types::DateTime;
use chrono::Utc;
use lambda_runtime::{Error, LambdaEvent};
use serde::Deserialize;
use std::env;
use tracing::{debug, instrument};

use crate::monitor::Monitor;

/// Incoming message structure for the Lambda event
#[derive(Deserialize, Debug)]
pub struct Event {
    pub clients: Vec<String>,
    pub provers: Vec<String>,
}

/// Lambda function configuration read from environment variables
struct Config {
    db_url: String,
    region: String,
    namespace: String,
}

impl Config {
    /// Load configuration from environment variables
    fn from_env() -> Result<Self, Error> {
        let db_url = env::var("DB_URL")
            .context("DB_URL environment variable is required")
            .map_err(|e| Error::from(e.to_string()))?;

        let region = env::var("AWS_REGION").unwrap_or_else(|_| "us-west-2".to_string());

        let namespace =
            env::var("CLOUDWATCH_NAMESPACE").unwrap_or_else(|_| "indexer-monitor".to_string());

        Ok(Self { db_url, region, namespace })
    }
}

/// Main Lambda handler function
#[instrument(skip_all, err)]
pub async fn function_handler(event: LambdaEvent<Event>) -> Result<(), Error> {
    debug!("Lambda function started");

    let config = Config::from_env()?;

    let event = event.payload;
    debug!(?event, "Received event");

    let monitor = Monitor::new(&config.db_url)
        .await
        .context("Failed to create monitor")
        .map_err(|e| Error::from(e.to_string()))?;

    let now = Utc::now().timestamp();
    let start_time = monitor
        .get_last_run()
        .await
        .context("Failed to get last run time")
        .map_err(|e| Error::from(e.to_string()))?;

    let mut metrics = vec![];

    debug!(start_time, now, "Fetching metrics");

    let expired = monitor
        .fetch_requests_expired(start_time, now)
        .await
        .context("Failed to fetch expired requests")
        .map_err(|e| Error::from(e.to_string()))?;

    let expired_count = expired.len();
    debug!(count = expired_count, "Found expired requests");
    metrics.push(new_metric("expired_requests_number", expired_count as f64, now));

    let requests_count = monitor
        .fetch_requests_number(start_time, now)
        .await
        .context("Failed to fetch requests number")
        .map_err(|e| Error::from(e.to_string()))?;
    debug!(count = requests_count, "Fetched requests number");
    metrics.push(new_metric("requests_number", requests_count as f64, now));

    let fulfillment_count = monitor
        .fetch_fulfillments_number(start_time, now)
        .await
        .context("Failed to fetch fulfilled requests number")
        .map_err(|e| Error::from(e.to_string()))?;
    debug!(count = fulfillment_count, "Fetched fulfilled requests number");
    metrics.push(new_metric("fulfilled_requests_number", fulfillment_count as f64, now));

    let slashed_count = monitor
        .fetch_slashed_number(start_time, now)
        .await
        .context("Failed to fetch slashed requests number")
        .map_err(|e| Error::from(e.to_string()))?;
    debug!(count = slashed_count, "Fetched slashed requests number");
    metrics.push(new_metric("slashed_requests_number", slashed_count as f64, now));

    for client in event.clients {
        debug!(client, "Processing client");
        let address = Address::from_str(&client)
            .context("Failed to parse client address")
            .map_err(|e| Error::from(e.to_string()))?;

        let expired_requests = monitor
            .fetch_requests_expired_from(start_time, now, address)
            .await
            .context("Failed to fetch expired requests for client {client}")
            .map_err(|e| Error::from(e.to_string()))?;
        let expired_count = expired_requests.len();
        metrics.push(new_metric(
            &format!("expired_requests_number_from_{client}"),
            expired_count as f64,
            now,
        ));

        let requests_count = monitor
            .fetch_requests_number_from_client(start_time, now, address)
            .await
            .context("Failed to fetch requests number for client {client}")
            .map_err(|e| Error::from(e.to_string()))?;
        metrics.push(new_metric(
            &format!("requests_number_from_{client}"),
            requests_count as f64,
            now,
        ));

        let fulfilled_count = monitor
            .fetch_fulfillments_number_from_client(start_time, now, address)
            .await
            .context("Failed to fetch fulfilled requests number for client {client}")
            .map_err(|e| Error::from(e.to_string()))?;
        metrics.push(new_metric(
            &format!("fulfilled_requests_number_from_{client}"),
            fulfilled_count as f64,
            now,
        ));
    }

    for prover in event.provers {
        debug!(prover, "Processing prover");

        let address = Address::from_str(&prover)
            .context("Failed to parse prover address")
            .map_err(|e| Error::from(e.to_string()))?;

        let fulfilled_number = monitor
            .fetch_fulfillments_number_by_prover(start_time, now, address)
            .await
            .context("Failed to fetch fulfilled requests number by prover {prover}")
            .map_err(|e| Error::from(e.to_string()))?;
        metrics.push(new_metric(
            &format!("fulfilled_requests_number_by_{prover}"),
            fulfilled_number as f64,
            now,
        ));

        let locked_number = monitor
            .fetch_locked_number_by_prover(start_time, now, address)
            .await
            .context("Failed to fetch locked requests number by prover {prover}")
            .map_err(|e| Error::from(e.to_string()))?;
        metrics.push(new_metric(
            &format!("locked_requests_number_by_{prover}"),
            locked_number as f64,
            now,
        ));

        let slashed_number = monitor
            .fetch_slashed_number_by_prover(start_time, now, address)
            .await
            .context("Failed to fetch slashed requests number by prover {prover}")
            .map_err(|e| Error::from(e.to_string()))?;
        metrics.push(new_metric(
            &format!("slashed_requests_number_by_{prover}"),
            slashed_number as f64,
            now,
        ));
    }

    debug!("Publishing metrics to CloudWatch");
    publish_metric(&config.region, &config.namespace, metrics)
        .await
        .map_err(|e| Error::from(e.to_string()))?;

    debug!("Updating last run time: {now}");
    monitor
        .set_last_run(now)
        .await
        .context("Failed to update last run time")
        .map_err(|e| Error::from(e.to_string()))?;

    Ok(())
}

fn new_metric(name: &str, value: f64, timestamp: i64) -> MetricDatum {
    MetricDatum::builder()
        .metric_name(name)
        .timestamp(DateTime::from_secs(timestamp))
        .unit(StandardUnit::Count)
        .value(value)
        .build()
}

/// Publishes a metric to CloudWatch
#[instrument(skip(region, namespace), err)]
async fn publish_metric(
    region: &str,
    namespace: &str,
    metrics: Vec<MetricDatum>,
) -> Result<(), Error> {
    let retry_config = RetryConfigBuilder::new()
        .mode(RetryMode::Standard)
        .max_attempts(3)
        .initial_backoff(Duration::from_secs(1))
        .max_backoff(Duration::from_secs(20))
        .reconnect_mode(ReconnectMode::ReconnectOnTransientError)
        .build();
    let config = aws_config::from_env()
        .region(Region::new(region.to_string()))
        .retry_config(retry_config)
        .load()
        .await;

    let client = CloudWatchClient::new(&config);
    client
        .put_metric_data()
        .namespace(namespace)
        .set_metric_data(Some(metrics))
        .send()
        .await
        .context("Failed to put metric data")
        .map_err(|e| Error::from(e.to_string()))?;

    Ok(())
}
