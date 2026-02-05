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

//! Address rotation for the order generator.
//!
//! See [README.md](../README.md#address-rotation) for the full design.

use std::{
    collections::HashMap,
    fs,
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};

const STATE_VERSION: u32 = 1;

/// Persisted rotation state for restart reliability.
///
/// Stored as versioned JSON for forward compatibility. See `STATE_VERSION`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RotationState {
    #[serde(default = "default_version")]
    pub version: u32,
    pub last_used_index: usize,
    #[serde(default)]
    pub last_rotation_timestamp: u64,
    #[serde(default)]
    pub max_expires_at_by_index: HashMap<String, u64>,
    #[serde(default)]
    pub pending_withdrawal: Vec<PendingWithdrawal>,
}

fn default_version() -> u32 {
    STATE_VERSION
}

/// A pending withdrawal for an address we rotated away from.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingWithdrawal {
    pub index: usize,
    pub max_expires_at: u64,
}

impl Default for RotationState {
    fn default() -> Self {
        Self {
            version: STATE_VERSION,
            last_used_index: 0,
            last_rotation_timestamp: 0,
            max_expires_at_by_index: HashMap::new(),
            pending_withdrawal: Vec::new(),
        }
    }
}

impl RotationState {
    /// Load state from file. On parse error, returns default (first run).
    pub fn load(path: &Path) -> Self {
        match fs::read_to_string(path) {
            Ok(s) => match serde_json::from_str(&s) {
                Ok(state) => state,
                Err(e) => {
                    tracing::warn!("Failed to parse rotation state, starting fresh: {e}");
                    Self::default()
                }
            },
            Err(e) => {
                if e.kind() != std::io::ErrorKind::NotFound {
                    tracing::warn!("Failed to read rotation state, starting fresh: {e}");
                }
                Self::default()
            }
        }
    }

    /// Save state with atomic write (temp file + rename).
    pub fn save(&self, path: &Path) -> anyhow::Result<()> {
        let parent = path.parent().unwrap_or_else(|| Path::new("."));
        let filename = path.file_name().map(|f| f.to_string_lossy()).unwrap_or_default();
        let temp = parent.join(format!(".{filename}.tmp"));
        let json = serde_json::to_string_pretty(self)?;
        fs::write(&temp, json)?;
        fs::rename(temp, path)?;
        Ok(())
    }

    /// Compute current rotation index from timestamp.
    ///
    /// Uses `(now_secs / interval_secs) % n_keys`. Returns 0 if `n_keys` is 0.
    pub fn current_index(&self, now_secs: u64, interval_secs: u64, n_keys: usize) -> usize {
        if n_keys == 0 || interval_secs == 0 {
            return 0;
        }
        (now_secs / interval_secs) as usize % n_keys
    }

    /// Update max_expires_at for an index (merge with existing).
    pub fn update_expires_at(&mut self, index: usize, expires_at: u64) {
        let key = index.to_string();
        let entry = self.max_expires_at_by_index.entry(key).or_insert(0);
        *entry = (*entry).max(expires_at);
    }

    /// Add to pending_withdrawal, deduping by index (keep max max_expires_at).
    pub fn add_pending(&mut self, index: usize, max_expires_at: u64) {
        if let Some(entry) = self.pending_withdrawal.iter_mut().find(|e| e.index == index) {
            entry.max_expires_at = entry.max_expires_at.max(max_expires_at);
        } else {
            self.pending_withdrawal.push(PendingWithdrawal { index, max_expires_at });
        }
    }

    /// Get max_expires_at for an index.
    pub fn max_expires_at(&self, index: usize) -> u64 {
        self.max_expires_at_by_index.get(&index.to_string()).copied().unwrap_or(0)
    }

    /// Check if we can safely withdraw from this index (now > max_expires_at + buffer).
    pub fn can_withdraw(&self, index: usize, now_secs: u64, buffer_secs: u64) -> bool {
        let max_exp = self.max_expires_at(index);
        now_secs > max_exp.saturating_add(buffer_secs)
    }

    /// Remove a pending withdrawal by index.
    pub fn remove_pending(&mut self, index: usize) {
        self.pending_withdrawal.retain(|p| p.index != index);
    }
}

/// Current Unix timestamp in seconds.
pub fn now_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_current_index() {
        let state = RotationState::default();
        // 0-86399 -> index 0, 86400-172799 -> index 1, etc.
        assert_eq!(state.current_index(0, 86400, 3), 0);
        assert_eq!(state.current_index(86399, 86400, 3), 0);
        assert_eq!(state.current_index(86400, 86400, 3), 1);
        assert_eq!(state.current_index(172800, 86400, 3), 2);
        assert_eq!(state.current_index(259200, 86400, 3), 0);
    }

    #[test]
    fn test_can_withdraw() {
        let mut state = RotationState::default();
        state.update_expires_at(0, 1000);
        assert!(!state.can_withdraw(0, 1050, 60)); // 1050 < 1060
        assert!(state.can_withdraw(0, 1061, 60)); // 1061 > 1060
    }

    #[test]
    fn test_add_pending_dedupe() {
        let mut state = RotationState::default();
        state.add_pending(0, 1000);
        state.add_pending(0, 1500); // should update to 1500
        assert_eq!(state.pending_withdrawal.len(), 1);
        assert_eq!(state.pending_withdrawal[0].max_expires_at, 1500);
    }

    #[test]
    fn test_current_index_edge_cases() {
        let state = RotationState::default();
        assert_eq!(state.current_index(100, 86400, 0), 0);
        assert_eq!(state.current_index(100, 0, 3), 0);
    }

    #[test]
    fn test_remove_pending() {
        let mut state = RotationState::default();
        state.add_pending(0, 1000);
        state.add_pending(1, 2000);
        state.remove_pending(0);
        assert_eq!(state.pending_withdrawal.len(), 1);
        assert_eq!(state.pending_withdrawal[0].index, 1);
    }
}
