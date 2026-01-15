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

//! Chain utilities for blockchain operations

use alloy::providers::Provider;
use anyhow::Context;

/// Find the nearest block number before a given timestamp using binary search.
///
/// This function performs a two-phase search:
/// 1. Linear search backwards in chunks to find a block <= target timestamp
/// 2. Binary search within the chunk to find the exact nearest block
///
/// Returns the block number of the latest block with timestamp <= target timestamp.
pub async fn block_number_near_timestamp(
    provider: impl Provider,
    latest_block_number: u64,
    timestamp_seconds: u64,
) -> anyhow::Result<u64> {
    tracing::debug!("Searching for block with timestamp <= {}", timestamp_seconds);

    // Phase 1: Linear search backwards in chunks until we find a block <= target_timestamp
    const LINEAR_SEARCH_CHUNK_SIZE: u64 = 100000;
    let mut probe = latest_block_number;
    loop {
        let block = provider
            .get_block_by_number(probe.into())
            .await
            .with_context(|| format!("Failed to get block {}", probe))?
            .with_context(|| format!("Block {} not found", probe))?;

        let block_timestamp = block.header.timestamp;
        tracing::trace!("Linear search at block {probe}, timestamp {block_timestamp}");
        if block_timestamp <= timestamp_seconds {
            break;
        }

        probe = probe.saturating_sub(LINEAR_SEARCH_CHUNK_SIZE);
        if probe == 0 {
            return Ok(0);
        }
    }

    // Phase 2: binary search between [low, high] to find exact nearest block
    let mut high = u64::min(probe + LINEAR_SEARCH_CHUNK_SIZE, latest_block_number);
    let mut low = probe;
    while low < high {
        let mid = (low + high).div_ceil(2);
        let block = provider
            .get_block_by_number(mid.into())
            .await
            .with_context(|| format!("Failed to get block {}", mid))?
            .with_context(|| format!("Block {} not found", mid))?;

        let block_timestamp = block.header.timestamp;
        tracing::trace!("Binary search at block {mid}, timestamp {block_timestamp}");
        if block_timestamp <= timestamp_seconds {
            low = mid;
        } else {
            high = mid - 1;
        }
    }

    Ok(low)
}
