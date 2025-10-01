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
                1 => "https://api.boundless.market/",  // Ethereum mainnet
                11155111 => "https://api-sepolia.boundless.market/",  // Sepolia testnet
                _ => {
                    tracing::warn!("Unknown chain ID {}, defaulting to mainnet indexer", zkc_chain_id);
                    "https://api.boundless.market/"
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

        let response = self
            .client
            .get(url)
            .send()
            .await
            .context("Failed to fetch staking history")?;

        if !response.status().is_success() {
            anyhow::bail!("API error: {}", response.status());
        }

        response
            .json()
            .await
            .context("Failed to parse staking history response")
    }

    /// Get staking data for current epoch
    pub async fn get_current_epoch_staking(&self, epoch: u64) -> Result<EpochStakingResponse> {
        let url = self
            .base_url
            .join(&format!("v1/staking/epochs/{}", epoch))
            .context("Failed to build URL")?;

        let response = self
            .client
            .get(url)
            .send()
            .await
            .context("Failed to fetch epoch staking data")?;

        if !response.status().is_success() {
            anyhow::bail!("API error: {}", response.status());
        }

        response
            .json()
            .await
            .context("Failed to parse epoch staking response")
    }

    /// Get PoVW history for a specific address
    pub async fn get_povw_history(&self, address: Address) -> Result<PovwHistoryResponse> {
        let url = self
            .base_url
            .join(&format!("v1/povw/addresses/{:#x}", address))
            .context("Failed to build URL")?;

        let response = self
            .client
            .get(url)
            .send()
            .await
            .context("Failed to fetch PoVW history")?;

        if !response.status().is_success() {
            anyhow::bail!("API error: {}", response.status());
        }

        response
            .json()
            .await
            .context("Failed to parse PoVW history response")
    }

    /// Get PoVW data for current epoch
    pub async fn get_current_epoch_povw(&self, epoch: u64) -> Result<EpochPovwResponse> {
        let url = self
            .base_url
            .join(&format!("v1/povw/epochs/{}", epoch))
            .context("Failed to build URL")?;

        let response = self
            .client
            .get(url)
            .send()
            .await
            .context("Failed to fetch epoch PoVW data")?;

        if !response.status().is_success() {
            anyhow::bail!("API error: {}", response.status());
        }

        response
            .json()
            .await
            .context("Failed to parse epoch PoVW response")
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
    pub rank: u32,
    pub staker_address: String,
    pub epoch: u64,
    pub staked_amount: String,
    pub is_withdrawing: bool,
    pub rewards_delegated_to: Option<String>,
    pub votes_delegated_to: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct StakingSummary {
    pub staker_address: String,
    pub total_staked: String,
    pub is_withdrawing: bool,
    pub rewards_delegated_to: Option<String>,
    pub votes_delegated_to: Option<String>,
    pub epochs_participated: u32,
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
    pub num_stakers: u32,
    pub num_withdrawing: u32,
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
    pub total_emissions: String,
    pub total_capped_rewards: String,
    pub total_uncapped_rewards: String,
    pub epoch_start_time: u64,
    pub epoch_end_time: u64,
    pub num_participants: u32,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct EpochPovwResponse {
    pub entries: Vec<PovwEntry>,
    pub pagination: PaginationInfo,
    pub summary: Option<EpochPovwSummary>,
}

/// Information about claimable PoVW rewards
#[derive(Debug, Deserialize, Serialize)]
pub struct ClaimableRewards {
    pub claimable_amount: u64,
    pub epochs: Vec<u64>,
}

/// Information about the current epoch
#[derive(Debug, Deserialize, Serialize)]
pub struct EpochInfo {
    pub epoch_number: u64,
    pub status: String,
    pub start_timestamp: u64,
    pub end_timestamp: u64,
    pub total_work: Option<u64>,
    pub participants_count: Option<u32>,
}

impl IndexerClient {
    /// Get claimable PoVW rewards for an address
    pub async fn get_claimable_povw_rewards(&self, address: Address) -> Result<ClaimableRewards> {
        let url = self.base_url.join(&format!("api/v1/povw/claimable/{:#x}", address))?;

        let response = self.client
            .get(url)
            .send()
            .await
            .context("Failed to query claimable PoVW rewards")?;

        if !response.status().is_success() {
            bail!("Failed to get claimable rewards: HTTP {}", response.status());
        }

        response.json::<ClaimableRewards>()
            .await
            .context("Failed to parse claimable rewards response")
    }

    /// Get current epoch information
    pub async fn get_current_epoch_info(&self) -> Result<EpochInfo> {
        let url = self.base_url.join("api/v1/epochs/current")?;

        let response = self.client
            .get(url)
            .send()
            .await
            .context("Failed to query current epoch info")?;

        if !response.status().is_success() {
            bail!("Failed to get epoch info: HTTP {}", response.status());
        }

        response.json::<EpochInfo>()
            .await
            .context("Failed to parse epoch info response")
    }
}

/// Parse a string amount to U256
pub fn parse_amount(amount: &str) -> Result<U256> {
    U256::from_str_radix(amount, 10).context("Failed to parse amount")
}