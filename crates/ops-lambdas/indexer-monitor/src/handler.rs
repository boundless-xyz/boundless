// Copyright (c) 2025 RISC Zero, Inc.
//
// All rights reserved.

use std::str::FromStr;

use alloy::primitives::Address;
use anyhow::{Context, Result};
use aws_config::Region;
use aws_sdk_cloudwatch::types::{MetricDatum, StandardUnit};
use aws_sdk_cloudwatch::Client as CloudWatchClient;
use aws_smithy_types::DateTime;
use chrono::Utc;
use lambda_runtime::{Error, LambdaEvent};
use serde::Deserialize;
use std::env;
use tracing::{debug, error, instrument};

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
    interval_seconds: i64,
    region: String,
    namespace: String,
}

impl Config {
    /// Load configuration from environment variables
    fn from_env() -> Result<Self, Error> {
        let db_url = env::var("DB_URL")
            .context("DB_URL environment variable is required")
            .map_err(|e| Error::from(e.to_string()))?;

        let interval_seconds = env::var("INTERVAL_SECONDS")
            .context("INTERVAL_SECONDS environment variable is required")
            .map_err(|e| Error::from(e.to_string()))?
            .parse()
            .context("INTERVAL_SECONDS must be a valid integer")
            .map_err(|e| Error::from(e.to_string()))?;

        let region = env::var("AWS_REGION").unwrap_or_else(|_| "us-west-2".to_string());

        let namespace =
            env::var("CLOUDWATCH_NAMESPACE").unwrap_or_else(|_| "indexer-monitor".to_string());

        Ok(Self { db_url, interval_seconds, region, namespace })
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
    let start_time = now - config.interval_seconds;

    debug!(start_time, now, "Fetching expired requests");

    // Fetch expired requests
    let expired = monitor
        .fetch_requests_expired(start_time, now)
        .await
        .context("Failed to fetch expired requests")
        .map_err(|e| Error::from(e.to_string()))?;

    let expired_count = expired.len();
    debug!(count = expired_count, "Found expired requests");

    if let Err(e) = publish_metric(
        &config.region,
        &config.namespace,
        "expired_requests",
        expired_count as f64,
        now,
    )
    .await
    {
        error!(error = %e, "Failed to publish metric");
    };

    let requests_count = monitor
        .fetch_requests_number(start_time, now)
        .await
        .context("Failed to fetch requests number")
        .map_err(|e| Error::from(e.to_string()))?;
    debug!(count = requests_count, "Fetched requests number");

    if let Err(e) = publish_metric(
        &config.region,
        &config.namespace,
        "requests_number",
        requests_count as f64,
        now,
    )
    .await
    {
        error!(error = %e, "Failed to publish metric");
    };

    let fulfillment_count = monitor
        .fetch_fulfillments_number(start_time, now)
        .await
        .context("Failed to fetch fulfilled requests number")
        .map_err(|e| Error::from(e.to_string()))?;
    debug!(count = fulfillment_count, "Fetched fulfilled requests number");
    if let Err(e) = publish_metric(
        &config.region,
        &config.namespace,
        "fulfilled_requests_number",
        fulfillment_count as f64,
        now,
    )
    .await
    {
        error!(error = %e, "Failed to publish metric");
    };

    let slashed_count = monitor
        .fetch_slashed_number(start_time, now)
        .await
        .context("Failed to fetch slashed requests number")
        .map_err(|e| Error::from(e.to_string()))?;
    debug!(count = slashed_count, "Fetched slashed requests number");
    if let Err(e) = publish_metric(
        &config.region,
        &config.namespace,
        "slashed_requests_number",
        slashed_count as f64,
        now,
    )
    .await
    {
        error!(error = %e, "Failed to publish metric");
    };

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
        if let Err(e) = publish_metric(
            &config.region,
            &config.namespace,
            format!("expired_requests_from_{client}").as_str(),
            expired_count as f64,
            now,
        )
        .await
        {
            error!(error = %e, "Failed to publish metric");
        };

        let requests_count = monitor
            .fetch_requests_number_from_client(start_time, now, address)
            .await
            .context("Failed to fetch requests number for client {client}")
            .map_err(|e| Error::from(e.to_string()))?;
        if let Err(e) = publish_metric(
            &config.region,
            &config.namespace,
            format!("requests_number_from_{client}").as_str(),
            requests_count as f64,
            now,
        )
        .await
        {
            error!(error = %e, "Failed to publish metric");
        };

        let fulfilled_count = monitor
            .fetch_fulfillments_number_from_client(start_time, now, address)
            .await
            .context("Failed to fetch fulfilled requests number for client {client}")
            .map_err(|e| Error::from(e.to_string()))?;
        if let Err(e) = publish_metric(
            &config.region,
            &config.namespace,
            format!("fulfilled_requests_number_from_{client}").as_str(),
            fulfilled_count as f64,
            now,
        )
        .await
        {
            error!(error = %e, "Failed to publish metric");
        };
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
        if let Err(e) = publish_metric(
            &config.region,
            &config.namespace,
            format!("fulfilled_requests_number_by_{prover}").as_str(),
            fulfilled_number as f64,
            now,
        )
        .await
        {
            error!(error = %e, "Failed to publish metric");
        };

        let locked_number = monitor
            .fetch_locked_number_by_prover(start_time, now, address)
            .await
            .context("Failed to fetch locked requests number by prover {prover}")
            .map_err(|e| Error::from(e.to_string()))?;
        if let Err(e) = publish_metric(
            &config.region,
            &config.namespace,
            format!("locked_requests_number_by_{prover}").as_str(),
            locked_number as f64,
            now,
        )
        .await
        {
            error!(error = %e, "Failed to publish metric");
        };

        let slashed_number = monitor
            .fetch_slashed_number_by_prover(start_time, now, address)
            .await
            .context("Failed to fetch slashed requests number by prover {prover}")
            .map_err(|e| Error::from(e.to_string()))?;
        if let Err(e) = publish_metric(
            &config.region,
            &config.namespace,
            format!("slashed_requests_number_by_{prover}").as_str(),
            slashed_number as f64,
            now,
        )
        .await
        {
            error!(error = %e, "Failed to publish metric");
        };
    }

    Ok(())
}

/// Publishes a metric to CloudWatch
#[instrument(skip(region, namespace), err)]
async fn publish_metric(
    region: &str,
    namespace: &str,
    metric_name: &str,
    value: f64,
    timestamp: i64,
) -> Result<(), Error> {
    // Configure AWS SDK
    let config = aws_config::from_env().region(Region::new(region.to_string())).load().await;

    // Create CloudWatch client
    let client = CloudWatchClient::new(&config);

    // Create metric datum
    let datum = MetricDatum::builder()
        .metric_name(metric_name)
        .timestamp(DateTime::from_secs(timestamp))
        .unit(StandardUnit::Count)
        .value(value)
        .build();

    // Send metric to CloudWatch
    client
        .put_metric_data()
        .namespace(namespace)
        .metric_data(datum)
        .send()
        .await
        .context("Failed to put metric data")
        .map_err(|e| Error::from(e.to_string()))?;

    Ok(())
}
