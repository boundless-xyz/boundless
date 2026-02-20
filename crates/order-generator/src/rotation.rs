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

//! Address rotation state for the order generator.
//!
//! See [README.md](../README.md#address-rotation) for the full design.

use std::{
    fs,
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};

const STATE_VERSION: u32 = 2;

/// Persisted rotation state for restart reliability.
///
/// Tracks the currently active key index and the maximum request expiry seen
/// for that key. On rotation, the caller waits until `max_expires_at + buffer`
/// has passed before withdrawing, ensuring no in-flight requests are stranded.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RotationState {
    #[serde(default = "default_version")]
    pub version: u32,
    /// Index into the rotation key array currently used for submissions.
    pub current_index: usize,
    /// Maximum `expires_at` timestamp (block seconds) seen for the current key.
    /// Zero means no requests have been submitted from this key yet.
    #[serde(default)]
    pub max_expires_at: u64,
}

fn default_version() -> u32 {
    STATE_VERSION
}

impl Default for RotationState {
    fn default() -> Self {
        Self { version: STATE_VERSION, current_index: 0, max_expires_at: 0 }
    }
}

impl RotationState {
    /// Load state from file. Returns default on missing file or parse error.
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

    /// Atomically save state (write to temp file then rename).
    pub fn save(&self, path: &Path) -> anyhow::Result<()> {
        let parent = path.parent().unwrap_or_else(|| Path::new("."));
        let filename = path.file_name().map(|f| f.to_string_lossy()).unwrap_or_default();
        let temp = parent.join(format!(".{filename}.tmp"));
        let json = serde_json::to_string_pretty(self)?;
        fs::write(&temp, json)?;
        if let Err(e) = fs::rename(&temp, path) {
            let _ = fs::remove_file(&temp);
            return Err(e.into());
        }
        Ok(())
    }
}

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

    #[test]
    fn test_state_roundtrip() {
        let dir = std::env::temp_dir();
        let path = dir.join("test-rotation-state-v2.json");
        let state =
            RotationState { version: STATE_VERSION, current_index: 2, max_expires_at: 9999 };
        state.save(&path).unwrap();
        let loaded = RotationState::load(&path);
        assert_eq!(loaded.current_index, 2);
        assert_eq!(loaded.max_expires_at, 9999);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_load_missing_file_returns_default() {
        let path = std::path::Path::new("/tmp/nonexistent-rotation-state-xyz.json");
        let state = RotationState::load(path);
        assert_eq!(state.current_index, 0);
        assert_eq!(state.max_expires_at, 0);
    }
}
