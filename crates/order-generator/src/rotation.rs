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

//! Address rotation helpers for the order generator.
//!
//! See [README.md](../README.md#address-rotation) for the full design.

use std::time::{SystemTime, UNIX_EPOCH};

/// Compute the desired rotation index for the given block timestamp.
///
/// Uses `(now_secs / interval_secs) % n_keys`. Returns 0 for degenerate inputs.
pub fn desired_index(now_secs: u64, interval_secs: u64, n_keys: usize) -> usize {
    if n_keys == 0 || interval_secs == 0 {
        return 0;
    }
    usize::try_from(now_secs / interval_secs).unwrap_or(usize::MAX) % n_keys
}

/// Current Unix timestamp in seconds.
pub fn now_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_desired_index() {
        // 0-86399 → 0, 86400-172799 → 1, 172800-259199 → 2, 259200+ → wraps
        assert_eq!(desired_index(0, 86400, 3), 0);
        assert_eq!(desired_index(86399, 86400, 3), 0);
        assert_eq!(desired_index(86400, 86400, 3), 1);
        assert_eq!(desired_index(172800, 86400, 3), 2);
        assert_eq!(desired_index(259200, 86400, 3), 0);
    }

    #[test]
    fn test_desired_index_edge_cases() {
        assert_eq!(desired_index(100, 86400, 0), 0);
        assert_eq!(desired_index(100, 0, 3), 0);
    }
}
