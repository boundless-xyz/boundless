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

//! Epoch boundary calculation utilities.
//!
//! This module provides functions to compute epoch boundaries using the same logic
//! as the ZKC contract. Epochs are 2-day periods starting from a configurable
//! epoch 0 start time.

/// Epoch duration in seconds (2 days)
pub const EPOCH_DURATION: u64 = 2 * 24 * 60 * 60; // 172800 seconds

/// Default epoch 0 start time (production ZKC value)
pub const DEFAULT_EPOCH0_START_TIME: u64 = 1757536895;

/// Represents the boundaries of a single epoch
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EpochBoundary {
    pub epoch_number: u64,
    pub start_time: u64,
    pub end_time: u64,
}

/// Calculator for epoch boundaries using the same logic as the ZKC contract.
///
/// The ZKC contract defines epochs as follows:
/// - `getCurrentEpoch() = (block.timestamp - epoch0StartTime) / EPOCH_DURATION`
/// - `getEpochStartTime(epoch) = epoch0StartTime + (epoch * EPOCH_DURATION)`
/// - `getEpochEndTime(epoch) = getEpochStartTime(epoch + 1) - 1`
#[derive(Debug, Clone)]
pub struct EpochCalculator {
    epoch0_start_time: u64,
}

impl EpochCalculator {
    /// Create a new EpochCalculator with the given epoch 0 start time
    pub fn new(epoch0_start_time: u64) -> Self {
        Self { epoch0_start_time }
    }

    /// Create a new EpochCalculator with the default production epoch 0 start time
    pub fn with_default() -> Self {
        Self::new(DEFAULT_EPOCH0_START_TIME)
    }

    /// Get the epoch 0 start time
    pub fn epoch0_start_time(&self) -> u64 {
        self.epoch0_start_time
    }

    /// Get epoch number for a given timestamp.
    /// Returns None if the timestamp is before epoch 0.
    pub fn get_epoch_for_timestamp(&self, timestamp: u64) -> Option<u64> {
        if timestamp < self.epoch0_start_time {
            return None;
        }
        Some((timestamp - self.epoch0_start_time) / EPOCH_DURATION)
    }

    /// Get start time for an epoch
    pub fn get_epoch_start_time(&self, epoch: u64) -> u64 {
        self.epoch0_start_time + (epoch * EPOCH_DURATION)
    }

    /// Get end time for an epoch (last second of the epoch, inclusive)
    pub fn get_epoch_end_time(&self, epoch: u64) -> u64 {
        self.get_epoch_start_time(epoch + 1) - 1
    }

    /// Get epoch boundary for an epoch number
    pub fn get_epoch_boundary(&self, epoch: u64) -> EpochBoundary {
        EpochBoundary {
            epoch_number: epoch,
            start_time: self.get_epoch_start_time(epoch),
            end_time: self.get_epoch_end_time(epoch),
        }
    }

    /// Iterate over epochs in a time range (inclusive).
    /// Returns an iterator yielding EpochBoundary for each epoch that overlaps
    /// with the given time range.
    pub fn iter_epochs(
        &self,
        from_time: u64,
        to_time: u64,
    ) -> impl Iterator<Item = EpochBoundary> + '_ {
        let start_epoch = self.get_epoch_for_timestamp(from_time).unwrap_or(0);
        let end_epoch = self.get_epoch_for_timestamp(to_time).unwrap_or(start_epoch);

        (start_epoch..=end_epoch).map(|epoch| self.get_epoch_boundary(epoch))
    }

    /// Get all epochs up to and including the given timestamp.
    /// Returns an empty iterator if the timestamp is before epoch 0.
    pub fn iter_epochs_until(&self, to_time: u64) -> impl Iterator<Item = EpochBoundary> + '_ {
        let end_epoch = self.get_epoch_for_timestamp(to_time);

        match end_epoch {
            Some(end) => {
                let epochs: Vec<_> =
                    (0..=end).map(|epoch| self.get_epoch_boundary(epoch)).collect();
                epochs.into_iter()
            }
            None => Vec::new().into_iter(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_epoch_calculator_basic() {
        let epoch0_start = 1000000;
        let calc = EpochCalculator::new(epoch0_start);

        // Epoch 0 boundaries
        assert_eq!(calc.get_epoch_start_time(0), epoch0_start);
        assert_eq!(calc.get_epoch_end_time(0), epoch0_start + EPOCH_DURATION - 1);

        // Epoch 1 boundaries
        assert_eq!(calc.get_epoch_start_time(1), epoch0_start + EPOCH_DURATION);
        assert_eq!(calc.get_epoch_end_time(1), epoch0_start + 2 * EPOCH_DURATION - 1);
    }

    #[test]
    fn test_get_epoch_for_timestamp() {
        let epoch0_start = 1000000;
        let calc = EpochCalculator::new(epoch0_start);

        // Before epoch 0
        assert_eq!(calc.get_epoch_for_timestamp(epoch0_start - 1), None);

        // Epoch 0
        assert_eq!(calc.get_epoch_for_timestamp(epoch0_start), Some(0));
        assert_eq!(calc.get_epoch_for_timestamp(epoch0_start + EPOCH_DURATION - 1), Some(0));

        // Epoch 1
        assert_eq!(calc.get_epoch_for_timestamp(epoch0_start + EPOCH_DURATION), Some(1));
        assert_eq!(calc.get_epoch_for_timestamp(epoch0_start + 2 * EPOCH_DURATION - 1), Some(1));

        // Epoch 2
        assert_eq!(calc.get_epoch_for_timestamp(epoch0_start + 2 * EPOCH_DURATION), Some(2));
    }

    #[test]
    fn test_get_epoch_boundary() {
        let epoch0_start = 1000000;
        let calc = EpochCalculator::new(epoch0_start);

        let boundary = calc.get_epoch_boundary(0);
        assert_eq!(boundary.epoch_number, 0);
        assert_eq!(boundary.start_time, epoch0_start);
        assert_eq!(boundary.end_time, epoch0_start + EPOCH_DURATION - 1);

        let boundary = calc.get_epoch_boundary(5);
        assert_eq!(boundary.epoch_number, 5);
        assert_eq!(boundary.start_time, epoch0_start + 5 * EPOCH_DURATION);
        assert_eq!(boundary.end_time, epoch0_start + 6 * EPOCH_DURATION - 1);
    }

    #[test]
    fn test_iter_epochs() {
        let epoch0_start = 1000000;
        let calc = EpochCalculator::new(epoch0_start);

        // Iterate over epochs 0-2
        let from_time = epoch0_start;
        let to_time = epoch0_start + 2 * EPOCH_DURATION + 100;
        let epochs: Vec<_> = calc.iter_epochs(from_time, to_time).collect();

        assert_eq!(epochs.len(), 3);
        assert_eq!(epochs[0].epoch_number, 0);
        assert_eq!(epochs[1].epoch_number, 1);
        assert_eq!(epochs[2].epoch_number, 2);
    }

    #[test]
    fn test_iter_epochs_before_epoch0() {
        let epoch0_start = 1000000;
        let calc = EpochCalculator::new(epoch0_start);

        // Both times before epoch 0 - should still return epoch 0
        let epochs: Vec<_> = calc.iter_epochs(0, epoch0_start - 1).collect();
        assert_eq!(epochs.len(), 1);
        assert_eq!(epochs[0].epoch_number, 0);
    }

    #[test]
    fn test_epoch_duration_is_2_days() {
        // Verify EPOCH_DURATION matches 2 days in seconds
        assert_eq!(EPOCH_DURATION, 2 * 24 * 60 * 60);
        assert_eq!(EPOCH_DURATION, 172800);
    }
}
