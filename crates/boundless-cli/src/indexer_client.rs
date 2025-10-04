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

//! Client for interacting with the Boundless Indexer API.

#![allow(missing_docs)]

use alloy::primitives::{Address, U256};
use anyhow::{bail, Context, Result};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use url::Url;

/// Client for the Boundless Indexer API
pub struct IndexerClient {
    client: Client,
    base_url: Url,
}

impl IndexerClient {
    /// Create a new IndexerClient with hardcoded URL based on chain ID
    /// Can be overridden with INDEXER_API_URL environment variable
    pub fn new_from_chain_id(zkc_chain_id: u64) -> Result<Self> {
        // Check for environment variable override first
        let base_url = if let Ok(url_str) = std::env::var("INDEXER_API_URL") {
            tracing::debug!("Using INDEXER_API_URL from environment: {}", url_str);
            Url::parse(&url_str).context("Invalid INDEXER_API_URL")?
        } else {
            // Use hardcoded URLs based on chain ID
            let url_str = match zkc_chain_id {
                1 => "https://dk96ouq3dipzx.cloudfront.net/", // Ethereum mainnet
                11155111 => "https://api-sepolia.boundless.market/", // Sepolia testnet
                _ => {
                    tracing::warn!(
                        "Unknown chain ID {}, defaulting to mainnet indexer",
                        zkc_chain_id
                    );
                    "https://dk96ouq3dipzx.cloudfront.net/"
                }
            };
            tracing::debug!("Using indexer API for chain {}: {}", zkc_chain_id, url_str);
            Url::parse(url_str).expect("Hardcoded URL should be valid")
        };

        Self::new(base_url)
    }

    /// Create a new IndexerClient with explicit URL
    pub fn new(base_url: Url) -> Result<Self> {
        let client = Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .user_agent("boundless-cli/1.0")
            .build()
            .context("Failed to build HTTP client")?;

        Ok(Self { client, base_url })
    }

    /// Get staking history for a specific address
    pub async fn get_staking_history(&self, address: Address) -> Result<StakingHistoryResponse> {
        let url = self
            .base_url
            .join(&format!("v1/staking/addresses/{:#x}", address))
            .context("Failed to build URL")?;

        let response =
            self.client.get(url).send().await.context("Failed to fetch staking history")?;

        if !response.status().is_success() {
            anyhow::bail!("API error: {}", response.status());
        }

        response.json().await.context("Failed to parse staking history response")
    }

    /// Get staking summary for a specific epoch
    pub async fn get_epoch_staking(&self, epoch: u64) -> Result<EpochStakingSummary> {
        let url = self
            .base_url
            .join(&format!("v1/staking/epochs/{}", epoch))
            .context("Failed to build URL")?;

        let response = self
            .client
            .get(url.clone())
            .send()
            .await
            .with_context(|| format!("Failed to fetch epoch staking data from {}", url))?;

        if !response.status().is_success() {
            bail!("API error from {}: {}", url, response.status());
        }

        response
            .json()
            .await
            .with_context(|| format!("Failed to parse epoch staking response from {}", url))
    }

    /// Get PoVW history for a specific address
    pub async fn get_povw_history(&self, address: Address) -> Result<PovwHistoryResponse> {
        let url = self
            .base_url
            .join(&format!("v1/povw/addresses/{:#x}", address))
            .context("Failed to build URL")?;

        let response = self.client.get(url).send().await.context("Failed to fetch PoVW history")?;

        if !response.status().is_success() {
            anyhow::bail!("API error: {}", response.status());
        }

        response.json().await.context("Failed to parse PoVW history response")
    }

    /// Get PoVW summary for a specific epoch
    pub async fn get_epoch_povw(&self, epoch: u64) -> Result<EpochPovwSummary> {
        let url = self
            .base_url
            .join(&format!("v1/povw/epochs/{}", epoch))
            .context("Failed to build URL")?;

        let response = self
            .client
            .get(url.clone())
            .send()
            .await
            .with_context(|| format!("Failed to fetch epoch PoVW data from {}", url))?;

        if !response.status().is_success() {
            bail!("API error from {}: {}", url, response.status());
        }

        response
            .json()
            .await
            .with_context(|| format!("Failed to parse epoch PoVW response from {}", url))
    }
}

// Data Models

#[derive(Debug, Deserialize, Serialize)]
pub struct PaginationInfo {
    pub count: u32,
    pub offset: u32,
    pub limit: u32,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct StakingEntry {
    pub staker_address: String,
    pub epoch: u64,
    pub staked_amount: String,
    pub staked_amount_formatted: String,
    pub is_withdrawing: bool,
    pub rewards_generated: String,
    pub rewards_generated_formatted: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rank: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rewards_delegated_to: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub votes_delegated_to: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct StakingSummary {
    pub staker_address: String,
    pub total_staked: String,
    pub total_staked_formatted: String,
    pub is_withdrawing: bool,
    pub epochs_participated: u64,
    pub total_rewards_generated: String,
    pub total_rewards_generated_formatted: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rewards_delegated_to: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub votes_delegated_to: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct StakingHistoryResponse {
    pub entries: Vec<StakingEntry>,
    pub pagination: PaginationInfo,
    pub summary: Option<StakingSummary>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct EpochStakingSummary {
    pub epoch: u64,
    pub total_staked: String,
    pub total_staked_formatted: String,
    pub num_stakers: u64,
    pub num_withdrawing: u64,
    pub total_staking_emissions: String,
    pub total_staking_emissions_formatted: String,
    pub total_staking_power: String,
    pub total_staking_power_formatted: String,
    pub num_reward_recipients: u64,
    pub epoch_start_time: u64,
    pub epoch_end_time: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_updated_at: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct EpochStakingResponse {
    pub entries: Vec<StakingEntry>,
    pub pagination: PaginationInfo,
    pub summary: Option<EpochStakingSummary>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct PovwEntry {
    pub rank: u32,
    pub work_log_id: String,
    pub epoch: u64,
    pub work_submitted: String,
    pub percentage: f64,
    pub uncapped_rewards: String,
    pub reward_cap: String,
    pub actual_rewards: String,
    pub is_capped: bool,
    pub staked_amount: String,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct PovwSummary {
    pub work_log_id: String,
    pub total_work_submitted: String,
    pub total_actual_rewards: String,
    pub total_uncapped_rewards: String,
    pub epochs_participated: u32,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct PovwHistoryResponse {
    pub entries: Vec<PovwEntry>,
    pub pagination: PaginationInfo,
    pub summary: Option<PovwSummary>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct EpochPovwSummary {
    pub epoch: u64,
    pub total_work: String,
    pub total_work_formatted: String,
    pub total_emissions: String,
    pub total_emissions_formatted: String,
    pub total_capped_rewards: String,
    pub total_capped_rewards_formatted: String,
    pub total_uncapped_rewards: String,
    pub total_uncapped_rewards_formatted: String,
    pub epoch_start_time: u64,
    pub epoch_end_time: u64,
    pub num_participants: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_updated_at: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct EpochPovwResponse {
    pub entries: Vec<PovwEntry>,
    pub pagination: PaginationInfo,
    pub summary: Option<EpochPovwSummary>,
}

/// Parse a string amount to U256
pub fn parse_amount(amount: &str) -> Result<U256> {
    U256::from_str_radix(amount, 10).context("Failed to parse amount")
}
