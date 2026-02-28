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

use std::collections::VecDeque;

use alloy::primitives::B256;
use boundless_market::dynamic_gas_filler::PriorityMode;

#[derive(Clone, Debug)]
pub(crate) struct BlockHistoryEntry {
    // Stored for future reorg detection; not yet read outside tests.
    #[allow(dead_code)]
    pub block_number: u64,
    #[allow(dead_code)]
    pub block_hash: B256,
    #[allow(dead_code)]
    pub parent_hash: B256,
    pub base_fee_per_gas: Option<u128>,
    /// effective_gas_price - base_fee_per_gas for each tx in the block.
    pub priority_fees: Vec<u128>,
}

pub(crate) struct BlockHistory {
    entries: VecDeque<BlockHistoryEntry>,
    max_size: usize,
}

impl BlockHistory {
    pub fn new(max_size: usize) -> Self {
        Self { entries: VecDeque::with_capacity(max_size), max_size }
    }

    pub fn push(&mut self, entry: BlockHistoryEntry) {
        if self.entries.len() >= self.max_size {
            self.entries.pop_front();
        }
        self.entries.push_back(entry);
    }

    /// Estimate max_fee_per_gas from local block history, replicating PriorityMode logic.
    /// Returns None if history is empty or no base_fee data available.
    pub fn estimate_gas_price(&self, mode: &PriorityMode) -> Option<u128> {
        let latest = self.entries.back()?;
        let base_fee = latest.base_fee_per_gas?;
        let config = mode.config();

        // Collect all non-zero priority fees across the window
        let mut all_priority_fees: Vec<u128> = self
            .entries
            .iter()
            .flat_map(|b| b.priority_fees.iter().copied())
            .filter(|&fee| fee > 0)
            .collect();

        let priority_fee = if all_priority_fees.is_empty() {
            // Match alloy's EIP1559_MIN_PRIORITY_FEE fallback
            alloy::providers::utils::EIP1559_MIN_PRIORITY_FEE
        } else {
            all_priority_fees.sort_unstable();
            let idx = ((all_priority_fees.len() as f64) * config.priority_fee_percentile / 100.0)
                as usize;
            let idx = idx.min(all_priority_fees.len() - 1);
            let percentile_fee = all_priority_fees[idx];
            std::cmp::max(percentile_fee, alloy::providers::utils::EIP1559_MIN_PRIORITY_FEE)
        };

        let scaled_base =
            base_fee.saturating_mul(config.base_fee_multiplier_percentage as u128) / 100;
        let scaled_priority =
            priority_fee.saturating_mul(config.priority_fee_multiplier_percentage as u128) / 100;

        Some(scaled_base + scaled_priority)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(block_number: u64, base_fee: u128, priority_fees: Vec<u128>) -> BlockHistoryEntry {
        BlockHistoryEntry {
            block_number,
            block_hash: B256::ZERO,
            parent_hash: B256::ZERO,
            base_fee_per_gas: Some(base_fee),
            priority_fees,
        }
    }

    #[test]
    fn empty_history_returns_none() {
        let history = BlockHistory::new(20);
        assert!(history.estimate_gas_price(&PriorityMode::Medium).is_none());
    }

    #[test]
    fn sliding_window_evicts_oldest() {
        let mut history = BlockHistory::new(3);
        for i in 0..5 {
            history.push(entry(i, 100, vec![10]));
        }
        assert_eq!(history.entries.len(), 3);
        assert_eq!(history.entries.front().unwrap().block_number, 2);
    }

    #[test]
    fn gas_estimate_uses_base_fee_and_priority_percentile() {
        let mut history = BlockHistory::new(20);
        // 5 blocks, each with a range of priority fees
        for i in 0..5 {
            history.push(entry(
                i,
                1_000_000_000,
                vec![100_000_000, 200_000_000, 300_000_000, 400_000_000, 500_000_000],
            ));
        }
        let estimate = history.estimate_gas_price(&PriorityMode::Medium).unwrap();
        // Medium: base_fee * 200% + priority_fee_at_30th_percentile * 100%
        // base_fee = 1 gwei, 200% = 2 gwei
        // 25 priority fees sorted, 30th percentile index = floor(25 * 0.30) = 7 -> 200_000_000
        // estimate = 2_000_000_000 + 200_000_000 = 2_200_000_000
        assert!(estimate > 0);
        assert!(estimate >= 2_000_000_000);
        assert!(estimate <= 2_500_000_000);
    }

    #[test]
    fn no_priority_fees_uses_minimum() {
        let mut history = BlockHistory::new(20);
        history.push(entry(0, 1_000_000_000, vec![]));
        let estimate = history.estimate_gas_price(&PriorityMode::Medium).unwrap();
        // Should use EIP1559_MIN_PRIORITY_FEE (alloy default, typically 1 gwei)
        assert!(estimate > 0);
    }
}
