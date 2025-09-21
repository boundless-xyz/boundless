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

use std::collections::HashMap;
use std::sync::Arc;

use alloy::{
    primitives::{Address, U256}, providers::{
        fillers::{ChainIdFiller, FillProvider, JoinFill},
        Identity, Provider, ProviderBuilder, RootProvider,
    }, rpc::client::RpcClient, transports::layers::RetryBackoffLayer
};
use anyhow::{Context, Result};
use boundless_rewards::{
    compute_povw_rewards_by_work_log_id, fetch_all_event_logs, MAINNET_FROM_BLOCK,
    SEPOLIA_FROM_BLOCK,
};
use boundless_povw::deployments::Deployment as PovwDeployment;
use boundless_zkc::{contracts::IZKC, deployments::Deployment as ZkcDeployment};
use tokio::time::Duration;
use url::Url;

use crate::db::{
    rewards::{PovwRewardAggregate, PovwRewardByEpoch, RewardsDb, RewardsDbObj},
};

const EPOCHS_TO_PROCESS: u64 = 10;

#[derive(Clone)]
pub struct RewardsIndexerServiceConfig {
    pub interval: Duration,
    pub retries: u32,
    pub start_block: Option<u64>,
}

type ProviderType = FillProvider<JoinFill<Identity, ChainIdFiller>, RootProvider>;

pub struct RewardsIndexerService {
    provider: ProviderType,
    db: RewardsDbObj,
    zkc_address: Address,
    #[allow(dead_code)]
    vezkc_address: Address,
    #[allow(dead_code)]
    povw_accounting_address: Address,
    config: RewardsIndexerServiceConfig,
    chain_id: u64,
}

impl RewardsIndexerService {
    pub async fn new(
        rpc_url: Url,
        vezkc_address: Address,
        zkc_address: Address,
        povw_accounting_address: Address,
        db_conn: &str,
        config: RewardsIndexerServiceConfig,
    ) -> Result<Self> {
        let provider = ProviderBuilder::new()
            .disable_recommended_fillers()
            .filler(ChainIdFiller::default())
            .connect_client(RpcClient::builder().layer(RetryBackoffLayer::new(3, 1000, 200)).http(rpc_url));
        let chain_id = provider.get_chain_id().await?;
        let db: RewardsDbObj = Arc::new(RewardsDb::new(db_conn).await?);

        Ok(Self {
            provider,
            db,
            vezkc_address,
            zkc_address,
            povw_accounting_address,
            config,
            chain_id,
        })
    }

    pub async fn run(&mut self) -> Result<()> {
        // Get deployments based on chain ID
        let povw_deployment = PovwDeployment::from_chain_id(self.chain_id)
            .context("Could not determine PoVW deployment from chain ID")?;
        let zkc_deployment = ZkcDeployment::from_chain_id(self.chain_id)
            .context("Could not determine ZKC deployment from chain ID")?;

        // Determine starting block
        let start_block = if let Some(block) = self.config.start_block {
            block
        } else {
            match self.chain_id {
                1 => MAINNET_FROM_BLOCK,
                11155111 => SEPOLIA_FROM_BLOCK,
                _ => 0,
            }
        };

        // Get current block
        let current_block = self.provider.get_block_number().await?;

        tracing::info!(
            "Fetching events from block {} to {}",
            start_block,
            current_block
        );

        // Fetch all event logs
        let all_logs = fetch_all_event_logs(
            &self.provider,
            &povw_deployment,
            &zkc_deployment,
            start_block,
            current_block,
        )
        .await?;

        // Get current epoch from ZKC contract
        let zkc = IZKC::new(self.zkc_address, &self.provider);
        let current_epoch = zkc.getCurrentEpoch().call().await?;
        let current_epoch_u64 = current_epoch.to::<u64>();

        tracing::info!("Current epoch: {}", current_epoch_u64);

        // Store current epoch
        self.db.set_current_epoch(current_epoch_u64).await?;

        // Process the last EPOCHS_TO_PROCESS epochs
        let epochs_to_process = if current_epoch_u64 >= EPOCHS_TO_PROCESS {
            (current_epoch_u64 - EPOCHS_TO_PROCESS + 1..=current_epoch_u64).collect::<Vec<_>>()
        } else {
            (0..=current_epoch_u64).collect::<Vec<_>>()
        };

        for &epoch in &epochs_to_process {
            tracing::info!("Processing rewards for epoch {}", epoch);

            let epoch_rewards = compute_povw_rewards_by_work_log_id(
                &self.provider,
                &povw_deployment,
                self.zkc_address,
                U256::from(epoch),
                current_epoch,
                &all_logs.work_logs,
                &all_logs.epoch_finalized_logs,
            )
            .await?;

            // Convert to database format
            let mut db_rewards = Vec::new();
            let num_rewards = epoch_rewards.rewards_by_work_log_id.len();

            for (_, info) in epoch_rewards.rewards_by_work_log_id {
                let mut reward: PovwRewardByEpoch = info.into();
                reward.epoch = epoch;
                db_rewards.push(reward);
            }

            // Upsert epoch rewards
            self.db.upsert_povw_rewards_by_epoch(epoch, db_rewards).await?;
            tracing::info!(
                "Updated {} rewards for epoch {}",
                num_rewards,
                epoch
            );
        }

        // Compute and update aggregate table from all epochs
        tracing::info!("Computing aggregate rewards from all epochs");

        // Process all epochs from 0 to current
        let mut aggregate_map: HashMap<Address, PovwRewardAggregate> = HashMap::new();

        for epoch_num in 0..=current_epoch_u64 {
            let epoch_rewards = compute_povw_rewards_by_work_log_id(
                &self.provider,
                &povw_deployment,
                self.zkc_address,
                U256::from(epoch_num),
                current_epoch,
                &all_logs.work_logs,
                &all_logs.epoch_finalized_logs,
            )
            .await?;

            for (work_log_id, info) in epoch_rewards.rewards_by_work_log_id {
                let entry = aggregate_map.entry(work_log_id).or_insert_with(|| {
                    PovwRewardAggregate {
                        work_log_id,
                        total_work_submitted: U256::ZERO,
                        total_actual_rewards: U256::ZERO,
                        total_uncapped_rewards: U256::ZERO,
                        epochs_participated: 0,
                    }
                });

                entry.total_work_submitted += info.work_submitted;
                entry.total_actual_rewards += info.actual_rewards;
                entry.total_uncapped_rewards += info.uncapped_rewards;
                if info.work_submitted > U256::ZERO {
                    entry.epochs_participated += 1;
                }
            }
        }

        // Convert to vector and upsert
        let aggregates: Vec<PovwRewardAggregate> = aggregate_map.into_values().collect();
        self.db.upsert_povw_rewards_aggregate(aggregates.clone()).await?;

        tracing::info!(
            "Updated aggregate rewards for {} work logs",
            aggregates.len()
        );

        // Save last processed block
        self.db.set_last_rewards_block(current_block).await?;

        Ok(())
    }
}