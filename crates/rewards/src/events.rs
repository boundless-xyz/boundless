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

//! Event fetching and log querying utilities.

use alloy::{
    primitives::B256,
    providers::Provider,
    rpc::types::{BlockNumberOrTag, Filter, Log},
    sol_types::SolEvent,
};
use anyhow::Context;
use boundless_povw::{deployments::Deployment, log_updater::IPovwAccounting};

/// Container for all event logs needed for rewards computation
#[derive(Debug)]
pub struct AllEventLogs {
    pub work_logs: Vec<Log>,
    pub epoch_finalized_logs: Vec<Log>,
    pub stake_created_logs: Vec<Log>,
    pub stake_added_logs: Vec<Log>,
    pub unstake_initiated_logs: Vec<Log>,
    pub unstake_completed_logs: Vec<Log>,
    pub vote_delegation_change_logs: Vec<Log>,
    pub reward_delegation_change_logs: Vec<Log>,
    pub vote_power_logs: Vec<Log>,
    pub reward_power_logs: Vec<Log>,
    pub povw_claims_logs: Vec<Log>,
    pub staking_claims_logs: Vec<Log>,
}

/// Query logs in chunks to avoid hitting provider limits
pub async fn query_logs_chunked<P: Provider>(
    provider: &P,
    filter: Filter,
    from_block: u64,
    to_block: u64,
    block_chunk_size: u64,
) -> anyhow::Result<Vec<Log>> {
    let mut all_logs = Vec::new();

    let mut current_from = from_block;
    while current_from <= to_block {
        let current_to = (current_from + block_chunk_size - 1).min(to_block);

        let chunk_filter = filter
            .clone()
            .from_block(BlockNumberOrTag::Number(current_from))
            .to_block(BlockNumberOrTag::Number(current_to));

        let logs = provider.get_logs(&chunk_filter).await?;
        all_logs.extend(logs);

        current_from = current_to + 1;
    }

    Ok(all_logs)
}

/// Fetch all event logs needed for rewards computation
pub async fn fetch_all_event_logs<P: Provider>(
    provider: &P,
    deployment: &Deployment,
    zkc_deployment: &boundless_zkc::deployments::Deployment,
    from_block_num: u64,
    current_block: u64,
    block_chunk_size: u64,
) -> anyhow::Result<AllEventLogs> {
    tracing::info!("Fetching blockchain event data ({} blocks)...", current_block - from_block_num);

    // Batch 1: Core stake and work data (5 parallel queries)
    tracing::info!("[1/2] Querying stake and work events...");

    let work_filter = Filter::new()
        .address(deployment.povw_accounting_address)
        .event_signature(IPovwAccounting::WorkLogUpdated::SIGNATURE_HASH);

    let epoch_finalized_filter = Filter::new()
        .address(deployment.povw_accounting_address)
        .event_signature(IPovwAccounting::EpochFinalized::SIGNATURE_HASH);

    let stake_created_filter = Filter::new().address(deployment.vezkc_address).event_signature(
        B256::from(alloy::primitives::keccak256("StakeCreated(uint256,address,uint256)")),
    );

    let stake_added_filter = Filter::new().address(deployment.vezkc_address).event_signature(
        B256::from(alloy::primitives::keccak256("StakeAdded(uint256,address,uint256,uint256)")),
    );

    let unstake_initiated_filter = Filter::new().address(deployment.vezkc_address).event_signature(
        B256::from(alloy::primitives::keccak256("UnstakeInitiated(uint256,address,uint256)")),
    );

    let (
        work_logs,
        epoch_finalized_logs,
        stake_created_logs,
        stake_added_logs,
        unstake_initiated_logs,
    ) = tokio::join!(
        async {
            query_logs_chunked(
                provider,
                work_filter.clone(),
                from_block_num,
                current_block,
                block_chunk_size,
            )
            .await
            .context("Failed to get work logs")
        },
        async {
            query_logs_chunked(
                provider,
                epoch_finalized_filter.clone(),
                from_block_num,
                current_block,
                block_chunk_size,
            )
            .await
            .context("Failed to get epoch finalized logs")
        },
        async {
            query_logs_chunked(
                provider,
                stake_created_filter.clone(),
                from_block_num,
                current_block,
                block_chunk_size,
            )
            .await
            .context("Failed to get stake created logs")
        },
        async {
            query_logs_chunked(
                provider,
                stake_added_filter.clone(),
                from_block_num,
                current_block,
                block_chunk_size,
            )
            .await
            .context("Failed to get stake added logs")
        },
        async {
            query_logs_chunked(
                provider,
                unstake_initiated_filter.clone(),
                from_block_num,
                current_block,
                block_chunk_size,
            )
            .await
            .context("Failed to get unstake initiated logs")
        }
    );

    // Batch 2: Delegation, completion, and reward claims (7 parallel queries)
    tracing::info!("[2/2] Querying delegation, unstake completion, and reward claim events...");

    let unstake_completed_filter = Filter::new().address(deployment.vezkc_address).event_signature(
        B256::from(alloy::primitives::keccak256("UnstakeCompleted(uint256,address,uint256)")),
    );

    let vote_delegation_change_filter =
        Filter::new().address(deployment.vezkc_address).event_signature(B256::from(
            alloy::primitives::keccak256("DelegateChanged(address,address,address)"),
        ));

    let reward_delegation_change_filter =
        Filter::new().address(deployment.vezkc_address).event_signature(B256::from(
            alloy::primitives::keccak256("RewardDelegateChanged(address,address,address)"),
        ));

    let vote_power_filter = Filter::new().address(deployment.vezkc_address).event_signature(
        B256::from(alloy::primitives::keccak256("DelegateVotesChanged(address,uint256,uint256)")),
    );

    let reward_power_filter = Filter::new().address(deployment.vezkc_address).event_signature(
        B256::from(alloy::primitives::keccak256("DelegateRewardsChanged(address,uint256,uint256)")),
    );

    let povw_claims_filter = Filter::new().address(zkc_deployment.zkc_address).event_signature(
        B256::from(alloy::primitives::keccak256("PoVWRewardsClaimed(address,uint256)")),
    );

    let staking_claims_filter = Filter::new().address(zkc_deployment.zkc_address).event_signature(
        B256::from(alloy::primitives::keccak256("StakingRewardsClaimed(address,uint256)")),
    );

    let (
        unstake_completed_logs,
        vote_delegation_change_logs,
        reward_delegation_change_logs,
        vote_power_logs,
        reward_power_logs,
        povw_claims_logs,
        staking_claims_logs,
    ) = tokio::join!(
        async {
            query_logs_chunked(
                provider,
                unstake_completed_filter.clone(),
                from_block_num,
                current_block,
                block_chunk_size,
            )
            .await
            .context("Failed to get unstake completed logs")
        },
        async {
            query_logs_chunked(
                provider,
                vote_delegation_change_filter.clone(),
                from_block_num,
                current_block,
                block_chunk_size,
            )
            .await
            .context("Failed to get vote delegation change logs")
        },
        async {
            query_logs_chunked(
                provider,
                reward_delegation_change_filter.clone(),
                from_block_num,
                current_block,
                block_chunk_size,
            )
            .await
            .context("Failed to get reward delegation change logs")
        },
        async {
            query_logs_chunked(
                provider,
                vote_power_filter.clone(),
                from_block_num,
                current_block,
                block_chunk_size,
            )
            .await
            .context("Failed to get vote power logs")
        },
        async {
            query_logs_chunked(
                provider,
                reward_power_filter.clone(),
                from_block_num,
                current_block,
                block_chunk_size,
            )
            .await
            .context("Failed to get reward power logs")
        },
        async {
            query_logs_chunked(
                provider,
                povw_claims_filter.clone(),
                from_block_num,
                current_block,
                block_chunk_size,
            )
            .await
            .context("Failed to get povw claims logs")
        },
        async {
            query_logs_chunked(
                provider,
                staking_claims_filter.clone(),
                from_block_num,
                current_block,
                block_chunk_size,
            )
            .await
            .context("Failed to get staking claims logs")
        }
    );

    tracing::info!("Event data fetched successfully");

    Ok(AllEventLogs {
        work_logs: work_logs?,
        epoch_finalized_logs: epoch_finalized_logs?,
        stake_created_logs: stake_created_logs?,
        stake_added_logs: stake_added_logs?,
        unstake_initiated_logs: unstake_initiated_logs?,
        unstake_completed_logs: unstake_completed_logs?,
        vote_delegation_change_logs: vote_delegation_change_logs?,
        reward_delegation_change_logs: reward_delegation_change_logs?,
        vote_power_logs: vote_power_logs?,
        reward_power_logs: reward_power_logs?,
        povw_claims_logs: povw_claims_logs?,
        staking_claims_logs: staking_claims_logs?,
    })
}
