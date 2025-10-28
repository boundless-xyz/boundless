// Copyright 2025 RISC Zero, Inc.
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

use std::time::{Duration, UNIX_EPOCH};

use alloy::primitives::{Address, B256, U256};
use anyhow::Result;
use boundless_market::contracts::{Offer, ProofRequest};
use chrono::{DateTime, Utc};
use clap::Args;
use colored::Colorize;

use crate::chain::block_number_near_timestamp;
use crate::config::{GlobalConfig, RequestorConfig};
use crate::config_ext::RequestorConfigExt;
use crate::display::{format_eth, network_name_from_chain_id, DisplayManager};

/// Get the status of a given request
#[derive(Args, Clone, Debug)]
pub struct RequestorStatus {
    /// The proof request identifier
    pub request_id: U256,

    /// The time at which the request expires, in seconds since the UNIX epoch
    pub expires_at: Option<u64>,

    /// Show detailed timeline and order parameters
    #[clap(short, long)]
    pub timeline: bool,

    /// Number of blocks to search backwards when order not in stream (default: 100000)
    #[clap(long)]
    pub search_blocks: Option<u64>,

    /// Override the starting block for event search
    #[clap(long)]
    pub search_start_block: Option<u64>,

    /// Override the ending block for event search
    #[clap(long)]
    pub search_end_block: Option<u64>,

    /// Requestor configuration (RPC URL, private key, deployment)
    #[clap(flatten)]
    pub requestor_config: RequestorConfig,
}

#[derive(Debug, Clone)]
enum TimelineEntry {
    Submitted {
        timestamp: DateTime<Utc>,
        block_number: Option<u64>,
        tx_hash: Option<B256>,
        request_digest: B256,
    },
    Locked {
        timestamp: u64,
        prover: Address,
        block_number: u64,
        tx_hash: B256,
        request_digest: B256,
    },
    LockTimeout {
        timestamp: u64,
    },
    RequestFulfilled {
        timestamp: u64,
        prover: Address,
        block_number: u64,
        tx_hash: B256,
        request_digest: B256,
    },
    ProofDelivered {
        timestamp: u64,
        prover: Address,
        block_number: u64,
        tx_hash: B256,
        request_digest: B256,
    },
    Slashed {
        timestamp: u64,
        collateral_burned: U256,
        collateral_transferred: U256,
        recipient: Address,
        block_number: u64,
        tx_hash: B256,
    },
    RequestTimeout {
        timestamp: u64,
    },
}

impl TimelineEntry {
    fn timestamp_seconds(&self) -> u64 {
        match self {
            TimelineEntry::Submitted { timestamp, .. } => timestamp.timestamp() as u64,
            TimelineEntry::Locked { timestamp, .. } => *timestamp,
            TimelineEntry::LockTimeout { timestamp } => *timestamp,
            TimelineEntry::RequestFulfilled { timestamp, .. } => *timestamp,
            TimelineEntry::ProofDelivered { timestamp, .. } => *timestamp,
            TimelineEntry::Slashed { timestamp, .. } => *timestamp,
            TimelineEntry::RequestTimeout { timestamp } => *timestamp,
        }
    }

    fn is_actual_event(&self) -> bool {
        match self {
            TimelineEntry::Submitted { .. }
            | TimelineEntry::Locked { .. }
            | TimelineEntry::RequestFulfilled { .. }
            | TimelineEntry::ProofDelivered { .. }
            | TimelineEntry::Slashed { .. } => true,
            TimelineEntry::LockTimeout { .. } | TimelineEntry::RequestTimeout { .. } => false,
        }
    }
}

impl RequestorStatus {
    /// Run the status command
    pub async fn run(&self, global_config: &GlobalConfig) -> Result<()> {
        let requestor_config = self.requestor_config.clone().load_and_validate()?;

        let client = requestor_config.client_builder(global_config.tx_timeout)?.build().await?;
        let status = client.boundless_market.get_status(self.request_id, self.expires_at).await?;

        let network_name = network_name_from_chain_id(client.deployment.market_chain_id);
        let display = DisplayManager::with_network(network_name);

        display.header("Request History");
        display.item_colored("Request ID", format!("{:#x}", self.request_id), "dimmed");

        let (status_text, status_color) = match status {
            boundless_market::contracts::RequestStatus::Fulfilled => ("✓ Fulfilled", "green"),
            boundless_market::contracts::RequestStatus::Locked => ("⏳ Locked", "yellow"),
            boundless_market::contracts::RequestStatus::Expired => ("✗ Expired", "yellow"),
            boundless_market::contracts::RequestStatus::Unknown => ("? Unknown", "dimmed"),
        };

        display.status("Status", status_text, status_color);

        if self.timeline {
            // Build timeline
            display.info("Fetching timeline...");
            let timeline = self.build_timeline(&client, &requestor_config).await?;

            if !timeline.is_empty() {
                self.display_timeline(&timeline);
            } else {
                // Check if we have order stream data
                let has_order = if let Some(ref offchain_client) = client.offchain_client {
                    offchain_client.fetch_order(self.request_id, None).await.is_ok()
                } else {
                    false
                };

                if !has_order {
                    let search_window = self.search_blocks.unwrap_or(100000);
                    println!("\n{}", "No events found".yellow().bold());
                    println!(
                        "  Searched the last {} blocks backwards from current block",
                        search_window.to_string().cyan()
                    );
                    println!("  Try using {} to search further back", "--search-blocks <N>".cyan());
                }
            }

            // Display order parameters if we have them
            if let Some(request) = self.get_proof_request(&client).await {
                self.display_order_parameters(&request, &display);
            }
        }

        Ok(())
    }

    /// Find the block range for event search.
    ///
    /// Returns (start_block, end_block) defining the search window.
    /// When order exists: uses submission to expiration timestamps.
    /// When no order: searches backwards from current block.
    async fn find_event_search_blocks<P, St, R, Si>(
        &self,
        client: &boundless_market::Client<P, St, R, Si>,
        order_data: &Option<(boundless_market::order_stream_client::Order, DateTime<Utc>)>,
    ) -> (u64, u64)
    where
        P: alloy::providers::Provider + Clone,
    {
        const DEFAULT_SEARCH_WINDOW: u64 = 100000;

        let deployment_block = client.deployment.deployment_block.unwrap_or(0);
        let latest_block = client
            .boundless_market
            .instance()
            .provider()
            .get_block_number()
            .await
            .unwrap_or(deployment_block.max(1));

        // Check for manual overrides first
        if self.search_start_block.is_some() || self.search_end_block.is_some() {
            let start_block = self.search_start_block.unwrap_or(deployment_block);
            let end_block = self.search_end_block.unwrap_or(latest_block);

            tracing::info!(
                "Using manually specified block range: {} to {}",
                start_block,
                end_block
            );

            // Validate and swap if needed
            if start_block > end_block {
                tracing::warn!(
                    "Start block {} is greater than end block {}, swapping them",
                    start_block,
                    end_block
                );
                return (end_block, start_block);
            }

            return (start_block, end_block);
        }

        if let Some((order, created_at)) = order_data {
            tracing::info!("Using order stream data to determine block range");

            let submission_time = UNIX_EPOCH + Duration::from_secs(created_at.timestamp() as u64);
            let expiration_timestamp =
                order.request.offer.rampUpStart + order.request.offer.timeout as u64;
            let expiration_time = UNIX_EPOCH + Duration::from_secs(expiration_timestamp);

            tracing::info!(
                "Submission time: {} (unix: {}), Expiration time: {} (unix: {})",
                created_at.format("%Y-%m-%d %H:%M:%S UTC"),
                created_at.timestamp(),
                DateTime::from_timestamp(expiration_timestamp as i64, 0)
                    .map(|dt| dt.format("%Y-%m-%d %H:%M:%S UTC").to_string())
                    .unwrap_or_else(|| "Invalid timestamp".to_string()),
                expiration_timestamp
            );

            let start_block = block_number_near_timestamp(
                client.boundless_market.instance().provider().clone(),
                latest_block,
                submission_time,
                Some(Duration::from_secs(3600)),
            )
            .await
            .unwrap_or(deployment_block);

            let end_block = block_number_near_timestamp(
                client.boundless_market.instance().provider().clone(),
                latest_block,
                expiration_time,
                Some(Duration::from_secs(3600)),
            )
            .await
            .unwrap_or(latest_block);

            tracing::info!(
                "Converted timestamps to blocks: submission -> block {}, expiration -> block {}",
                start_block,
                end_block
            );
            return (start_block, end_block);
        }

        let search_window = self.search_blocks.unwrap_or(DEFAULT_SEARCH_WINDOW);
        let start_block = latest_block.saturating_sub(search_window).max(deployment_block);
        let end_block = latest_block;

        tracing::info!(
            "No order stream data available, searching backwards {} blocks from current block {}",
            search_window,
            latest_block
        );
        tracing::info!("Backward search range: blocks {} to {}", start_block, end_block);
        (start_block, end_block)
    }

    async fn build_timeline<P, St, R, Si>(
        &self,
        client: &boundless_market::Client<P, St, R, Si>,
        _requestor_config: &crate::config::RequestorConfig,
    ) -> Result<Vec<TimelineEntry>>
    where
        P: alloy::providers::Provider + Clone,
    {
        let mut timeline = Vec::new();

        // Get EIP-712 domain for request digest computation
        let domain = client.boundless_market.eip712_domain().await?;

        // Query order stream once at the start
        let order_stream_order_data = if let Some(ref offchain_client) = client.offchain_client {
            tracing::debug!("Querying order stream for request info");
            offchain_client.fetch_order_with_timestamp(self.request_id, None).await.ok()
        } else {
            None
        };

        if let Some((ref _order, created_at)) = order_stream_order_data {
            tracing::info!("Found order in stream, created at {}", created_at);
        } else {
            tracing::info!("Order not found in stream");
        }

        // Get ProofRequest to compute request digest
        let proof_request = if let Some((ref order, _)) = order_stream_order_data {
            Some(order.request.clone())
        } else {
            self.get_proof_request(client).await
        };

        // Compute request digest if we have the proof request
        let request_digest = if let Some(ref request) = proof_request {
            request.signing_hash(domain.verifying_contract, domain.chain_id)?
        } else {
            // If we can't get the request, we can't compute the digest
            // This shouldn't happen according to user, but we need to handle it
            anyhow::bail!("Could not retrieve proof request to compute request digest")
        };

        let (lower_bound, upper_bound) =
            self.find_event_search_blocks(client, &order_stream_order_data).await;

        tracing::info!("Event search range determined: blocks {} to {}", lower_bound, upper_bound);

        let (submission_time, offer) = self
            .query_submission_info(client, &order_stream_order_data, lower_bound, upper_bound)
            .await;

        if let Some(timestamp) = submission_time {
            timeline.push(TimelineEntry::Submitted {
                timestamp,
                block_number: None,
                tx_hash: None,
                request_digest,
            });
        }

        // Query all events in parallel for better performance
        tracing::info!(
            "Querying events for request ID {:x} in blocks {} to {}",
            self.request_id,
            lower_bound,
            upper_bound
        );
        let (locked_result, delivered_result, fulfilled_result) = tokio::join!(
            client.boundless_market.query_request_locked_event(
                self.request_id,
                Some(lower_bound),
                Some(upper_bound)
            ),
            client.boundless_market.query_all_proof_delivered_events(
                self.request_id,
                Some(lower_bound),
                Some(upper_bound)
            ),
            client.boundless_market.query_request_fulfilled_event(
                self.request_id,
                Some(lower_bound),
                Some(upper_bound)
            ),
        );

        // Process RequestLocked result
        if let Ok(data) = locked_result {
            tracing::info!("Found RequestLocked event at block {}", data.block_number);
            if let Ok(Some(block)) = client
                .boundless_market
                .instance()
                .provider()
                .get_block_by_number(data.block_number.into())
                .await
            {
                let locked_request_digest =
                    data.event.request.signing_hash(domain.verifying_contract, domain.chain_id)?;
                timeline.push(TimelineEntry::Locked {
                    timestamp: block.header.timestamp,
                    prover: data.event.prover,
                    block_number: data.block_number,
                    tx_hash: data.tx_hash,
                    request_digest: locked_request_digest,
                });
            }
        }

        // Process ProofDelivered results
        if let Ok(events) = delivered_result {
            tracing::info!("Found {} ProofDelivered event(s)", events.len());
            for data in events.iter() {
                if let Ok(Some(block)) = client
                    .boundless_market
                    .instance()
                    .provider()
                    .get_block_by_number(data.block_number.into())
                    .await
                {
                    timeline.push(TimelineEntry::ProofDelivered {
                        timestamp: block.header.timestamp,
                        prover: data.event.prover,
                        block_number: data.block_number,
                        tx_hash: data.tx_hash,
                        request_digest: data.event.fulfillment.requestDigest,
                    });
                }
            }
        }

        // Process RequestFulfilled result
        if let Ok(data) = fulfilled_result {
            tracing::info!("Found RequestFulfilled event at block {}", data.block_number);
            if let Ok(Some(block)) = client
                .boundless_market
                .instance()
                .provider()
                .get_block_by_number(data.block_number.into())
                .await
            {
                timeline.push(TimelineEntry::RequestFulfilled {
                    timestamp: block.header.timestamp,
                    prover: data.event.prover,
                    block_number: data.block_number,
                    tx_hash: data.tx_hash,
                    request_digest: data.event.requestDigest,
                });
            }
        }

        // Add deadline milestones if we have offer parameters
        // Check order stream first, then fall back to queried offer
        let offer_params = order_stream_order_data
            .as_ref()
            .map(|(order, _)| &order.request.offer)
            .or(offer.as_ref());

        if let Some(offer) = offer_params {
            let lock_timeout = offer.rampUpStart + offer.lockTimeout as u64;
            let request_timeout = offer.rampUpStart + offer.timeout as u64;

            timeline.push(TimelineEntry::LockTimeout { timestamp: lock_timeout });
            timeline.push(TimelineEntry::RequestTimeout { timestamp: request_timeout });

            // Check if request was fulfilled before lock timeout
            let fulfilled_before_lock_timeout = timeline
                .iter()
                .any(|entry| matches!(entry, TimelineEntry::ProofDelivered { timestamp, .. } if *timestamp <= lock_timeout));

            // Query for slash events only if NOT fulfilled before lock timeout
            if !fulfilled_before_lock_timeout {
                // Calculate search window for slash events (after request expiration)
                let expiration_time = UNIX_EPOCH + Duration::from_secs(request_timeout);
                let slash_search_start = block_number_near_timestamp(
                    client.boundless_market.instance().provider().clone(),
                    upper_bound,
                    expiration_time,
                    Some(Duration::from_secs(3600)),
                )
                .await
                .unwrap_or(upper_bound);

                // Search for slash event after expiration
                tracing::info!(
                    "Querying ProverSlashed event for request ID {:x} in blocks {} to {} (after request expiration)",
                    self.request_id,
                    slash_search_start,
                    upper_bound
                );
                if let Ok(data) = client
                    .boundless_market
                    .query_prover_slashed_event(
                        self.request_id,
                        Some(slash_search_start),
                        Some(upper_bound),
                    )
                    .await
                {
                    tracing::info!("Found ProverSlashed event at block {}", data.block_number);
                    if let Ok(Some(block)) = client
                        .boundless_market
                        .instance()
                        .provider()
                        .get_block_by_number(data.block_number.into())
                        .await
                    {
                        timeline.push(TimelineEntry::Slashed {
                            timestamp: block.header.timestamp,
                            collateral_burned: data.event.collateralBurned,
                            collateral_transferred: data.event.collateralTransferred,
                            recipient: data.event.collateralRecipient,
                            block_number: data.block_number,
                            tx_hash: data.tx_hash,
                        });
                    }
                }
            }
        }

        // Sort timeline chronologically
        timeline.sort_by_key(|entry| entry.timestamp_seconds());

        Ok(timeline)
    }

    async fn query_submission_info<P, St, R, Si>(
        &self,
        client: &boundless_market::Client<P, St, R, Si>,
        order_data: &Option<(boundless_market::order_stream_client::Order, DateTime<Utc>)>,
        lower_bound: u64,
        upper_bound: u64,
    ) -> (Option<DateTime<Utc>>, Option<Offer>)
    where
        P: alloy::providers::Provider + Clone,
    {
        // Use order stream data if available
        if let Some((order, created_at)) = order_data {
            tracing::debug!("Using order stream data for submission info");
            return (Some(*created_at), Some(order.request.offer.clone()));
        }

        // Fallback to chain events for offer only
        tracing::debug!(
            "Searching for RequestSubmitted event in blocks {} to {}",
            lower_bound,
            upper_bound
        );
        if let Ok(data) = client
            .boundless_market
            .query_request_submitted_event(self.request_id, Some(lower_bound), Some(upper_bound))
            .await
        {
            tracing::debug!("Found RequestSubmitted event at block {}", data.block_number);
            return (None, Some(data.request.offer));
        }

        tracing::debug!("No RequestSubmitted event found in specified range");
        (None, None)
    }

    async fn get_proof_request<P, St, R, Si>(
        &self,
        client: &boundless_market::Client<P, St, R, Si>,
    ) -> Option<ProofRequest>
    where
        P: alloy::providers::Provider + Clone,
    {
        // Try order stream first if available
        if let Some(ref offchain_client) = client.offchain_client {
            if let Ok(order) = offchain_client.fetch_order(self.request_id, None).await {
                return Some(order.request);
            }
        }

        // Fallback to chain events
        if let Ok((request, _)) =
            client.boundless_market.get_submitted_request(self.request_id, None).await
        {
            return Some(request);
        }

        None
    }

    fn display_timeline(&self, timeline: &[TimelineEntry]) {
        println!("\n{}", "Timeline:".bold());

        for entry in timeline {
            let symbol = if entry.is_actual_event() { "⏺" } else { "⏰" };

            match entry {
                TimelineEntry::Submitted { timestamp, block_number, tx_hash, request_digest } => {
                    let formatted_time = format_timestamp(*timestamp);
                    let source_label = if block_number.is_some() && tx_hash.is_some() {
                        "(onchain)".dimmed()
                    } else {
                        "(offchain)".dimmed()
                    };
                    println!(
                        "  {} {}  {} {}",
                        symbol.cyan(),
                        "Submitted".bold(),
                        formatted_time,
                        source_label
                    );
                    println!("                 Request Digest: {:#x}", request_digest);
                }
                TimelineEntry::Locked {
                    timestamp,
                    prover,
                    block_number,
                    tx_hash,
                    request_digest,
                } => {
                    let formatted_time = format_timestamp_from_unix(*timestamp);
                    println!("  {} {}     {}", symbol.cyan(), "Locked".bold(), formatted_time);
                    println!("                 Prover: {}", format!("{:#x}", prover).dimmed());
                    println!(
                        "                 Block: {} | Tx: {:#x}",
                        block_number.to_string().dimmed(),
                        tx_hash
                    );
                    println!("                 Request Digest: {:#x}", request_digest);
                }
                TimelineEntry::LockTimeout { timestamp } => {
                    let formatted_time = format_timestamp_from_unix(*timestamp);
                    println!(
                        "  {} {}   {}",
                        symbol.yellow(),
                        "Lock Timeout".bold().yellow(),
                        formatted_time
                    );
                }
                TimelineEntry::RequestFulfilled {
                    timestamp,
                    prover,
                    block_number,
                    tx_hash,
                    request_digest,
                } => {
                    let formatted_time = format_timestamp_from_unix(*timestamp);
                    println!(
                        "  {} {} {}",
                        symbol.green(),
                        "Fulfilled".bold().green(),
                        formatted_time
                    );
                    println!("                 Prover: {}", format!("{:#x}", prover).dimmed());
                    println!(
                        "                 Block: {} | Tx: {:#x}",
                        block_number.to_string().dimmed(),
                        tx_hash
                    );
                    println!("                 Request Digest: {:#x}", request_digest);
                }
                TimelineEntry::ProofDelivered {
                    timestamp,
                    prover,
                    block_number,
                    tx_hash,
                    request_digest,
                } => {
                    let formatted_time = format_timestamp_from_unix(*timestamp);
                    println!(
                        "  {} {} {}",
                        symbol.cyan(),
                        "ProofDelivered".bold().cyan(),
                        formatted_time
                    );
                    println!("                 Prover: {}", format!("{:#x}", prover).dimmed());
                    println!(
                        "                 Block: {} | Tx: {:#x}",
                        block_number.to_string().dimmed(),
                        tx_hash
                    );
                    println!("                 Request Digest: {:#x}", request_digest);
                }
                TimelineEntry::Slashed {
                    timestamp,
                    collateral_burned,
                    collateral_transferred,
                    recipient,
                    block_number,
                    tx_hash,
                } => {
                    let formatted_time = format_timestamp_from_unix(*timestamp);
                    println!("  {} {}    {}", symbol.red(), "Slashed".bold().red(), formatted_time);
                    println!(
                        "                 Burned: {} HP",
                        format_eth(*collateral_burned).dimmed()
                    );
                    println!(
                        "                 Transferred: {} HP",
                        format_eth(*collateral_transferred).dimmed()
                    );
                    println!(
                        "                 Recipient: {}",
                        format!("{:#x}", recipient).dimmed()
                    );
                    println!(
                        "                 Block: {} | Tx: {:#x}",
                        block_number.to_string().dimmed(),
                        tx_hash
                    );
                }
                TimelineEntry::RequestTimeout { timestamp } => {
                    let formatted_time = format_timestamp_from_unix(*timestamp);
                    println!(
                        "  {} {} {}",
                        symbol.yellow(),
                        "Request Timeout".bold().yellow(),
                        formatted_time
                    );
                }
            }

            // Display block info if available (for submitted event which may or may not have it)
            if let TimelineEntry::Submitted { block_number, tx_hash, request_digest: _, .. } = entry
            {
                if let (Some(bn), Some(tx)) = (block_number, tx_hash) {
                    println!("                 Block: {} | Tx: {:#x}", bn.to_string().dimmed(), tx);
                }
            }
        }
    }

    fn display_order_parameters(&self, request: &ProofRequest, display: &DisplayManager) {
        println!("\n{}", "Order Parameters:".bold());

        let client_addr = request.client_address();
        display.item("Client", format!("{:#x}", client_addr).dimmed().to_string());
        display.item("Image URL", request.imageUrl.dimmed().to_string());

        let offer = &request.offer;
        display.item("Min Price", format!("{} ETH", format_eth(offer.minPrice)));
        display.item("Max Price", format!("{} ETH", format_eth(offer.maxPrice)));
        display.item("Lock Collateral", format!("{} ZKC", format_eth(offer.lockCollateral)));

        let lock_timeout_hrs = offer.lockTimeout / 3600;
        let lock_timeout_mins = (offer.lockTimeout % 3600) / 60;
        let lock_timeout_str = if lock_timeout_hrs > 0 {
            format!("{}h {}m", lock_timeout_hrs, lock_timeout_mins)
        } else {
            format!("{}m", lock_timeout_mins)
        };
        display.item("Lock Timeout", lock_timeout_str);

        let request_timeout_hrs = offer.timeout / 3600;
        let request_timeout_mins = (offer.timeout % 3600) / 60;
        let request_timeout_str = if request_timeout_hrs > 0 {
            format!("{}h {}m", request_timeout_hrs, request_timeout_mins)
        } else {
            format!("{}m", request_timeout_mins)
        };
        display.item("Request Timeout", request_timeout_str);
    }
}

fn format_timestamp(dt: DateTime<Utc>) -> String {
    let now = Utc::now();
    let duration = now.signed_duration_since(dt);

    let relative_str = if duration.num_seconds() < 0 {
        let abs_duration = dt.signed_duration_since(now);
        if abs_duration.num_seconds() < 60 {
            format!("in {}s", abs_duration.num_seconds())
        } else if abs_duration.num_minutes() < 60 {
            format!("in {}m", abs_duration.num_minutes())
        } else if abs_duration.num_hours() < 24 {
            format!("in {}h", abs_duration.num_hours())
        } else {
            format!("in {}d", abs_duration.num_days())
        }
    } else if duration.num_seconds() < 60 {
        format!("{}s ago", duration.num_seconds())
    } else if duration.num_minutes() < 60 {
        format!("{}m ago", duration.num_minutes())
    } else if duration.num_hours() < 24 {
        format!("{}h ago", duration.num_hours())
    } else {
        format!("{}d ago", duration.num_days())
    };

    format!("{} ({})", dt.format("%b %d, %Y %H:%M:%S"), relative_str.dimmed())
}

fn format_timestamp_from_unix(timestamp: u64) -> String {
    if let Some(dt) = DateTime::from_timestamp(timestamp as i64, 0) {
        format_timestamp(dt)
    } else {
        format!("{}", timestamp)
    }
}
