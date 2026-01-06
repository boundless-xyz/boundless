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

use alloy::primitives::Address;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use thiserror::Error;

pub const CURRENT_SCHEMA_VERSION: Version = Version { major: 1, minor: 0 };
pub const MIN_PRIORITY: i32 = 0;
pub const MAX_PRIORITY: i32 = 100;

#[derive(Debug, Error)]
pub enum ValidationError {
    #[error("Invalid schema version: expected {expected:?}, got {actual:?}")]
    InvalidSchemaVersion { expected: Version, actual: Version },
    #[error("Priority {priority} out of range [{min}, {max}]", priority = _0, min = MIN_PRIORITY, max = MAX_PRIORITY)]
    PriorityOutOfRange(i32),
    #[error("Duplicate requestor address: {0}")]
    DuplicateAddress(Address),
    #[error("Invalid mcycle range: min={min} max={max}")]
    InvalidMcycleRange { min: u64, max: u64 },
    #[error("Requestor name cannot be empty")]
    EmptyName,
    #[error("Failed to fetch list from URL: {0}")]
    FetchError(String),
    #[error("Failed to parse list: {0}")]
    ParseError(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Version {
    pub major: u32,
    pub minor: u32,
}

/// A requestor entry in the list
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RequestorEntry {
    /// Ethereum address of the requestor
    pub address: Address,
    /// Chain ID where this requestor operates
    #[serde(rename = "chainId")]
    pub chain_id: u64,
    /// Human-readable name for the requestor
    pub name: String,
    /// Optional description of the requestor
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Tags for categorization and filtering
    pub tags: Vec<String>,
    /// Optional extension metadata
    #[serde(default, skip_serializing_if = "Extensions::is_empty")]
    pub extensions: Extensions,
}

/// Extension metadata for requestor entries
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Extensions {
    /// Priority ranking for this requestor
    #[serde(skip_serializing_if = "Option::is_none")]
    pub priority: Option<PriorityExtension>,
    /// Estimated resource requirements for requests from this requestor
    #[serde(skip_serializing_if = "Option::is_none", rename = "requestEstimates")]
    pub request_estimates: Option<RequestEstimatesExtension>,
    /// Denylist metadata if this requestor should be blocked
    #[serde(skip_serializing_if = "Option::is_none")]
    pub denylist: Option<DenylistExtension>,
}

impl Extensions {
    pub fn is_empty(&self) -> bool {
        self.priority.is_none() && self.request_estimates.is_none() && self.denylist.is_none()
    }
}

/// Priority level for a requestor
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PriorityExtension {
    /// Priority level (0-100, higher = more priority)
    pub level: i32,
}

/// Estimated resource requirements for requests
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RequestEstimatesExtension {
    /// Estimated minimum mcycle count for requests from this requestor
    #[serde(rename = "estimatedMcycleCountMin")]
    pub estimated_mcycle_count_min: u64,
    /// Estimated maximum mcycle count for requests from this requestor
    #[serde(rename = "estimatedMcycleCountMax")]
    pub estimated_mcycle_count_max: u64,
    /// Estimated maximum input size in megabytes for requests from this requestor
    #[serde(rename = "estimatedMaxInputSizeMB")]
    pub estimated_max_input_size_mb: f64,
}

/// Denylist metadata for blocked requestors
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DenylistExtension {
    /// Reason for denylisting this requestor
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// ISO 8601 timestamp when blocking started
    #[serde(skip_serializing_if = "Option::is_none", rename = "blockedSince")]
    pub blocked_since: Option<String>,
}

/// A list of requestors
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RequestorList {
    /// Name of this list
    pub name: String,
    /// Description of this list's purpose
    pub description: String,
    /// Version of the schema this list conforms to
    #[serde(rename = "schemaVersion")]
    pub schema_version: Version,
    /// Version of this list content
    pub version: Version,
    /// List of requestor entries
    pub requestors: Vec<RequestorEntry>,
}

impl RequestorList {
    /// Create a new requestor list with the current schema version
    pub fn new(
        name: String,
        description: String,
        version: Version,
        requestors: Vec<RequestorEntry>,
    ) -> Self {
        Self { name, description, schema_version: CURRENT_SCHEMA_VERSION, version, requestors }
    }

    /// Validate the list according to schema rules
    pub fn validate(&self) -> Result<(), ValidationError> {
        // Check schema version compatibility
        if self.schema_version.major != CURRENT_SCHEMA_VERSION.major {
            return Err(ValidationError::InvalidSchemaVersion {
                expected: CURRENT_SCHEMA_VERSION,
                actual: self.schema_version,
            });
        }

        let mut seen_entries = HashSet::new();

        for entry in &self.requestors {
            // Check for duplicate (address, chain_id) pairs
            // Same address on different chains is allowed
            if !seen_entries.insert((entry.address, entry.chain_id)) {
                return Err(ValidationError::DuplicateAddress(entry.address));
            }

            // Check name is not empty
            if entry.name.trim().is_empty() {
                return Err(ValidationError::EmptyName);
            }

            // Validate priority extension if present
            if let Some(priority) = &entry.extensions.priority {
                if priority.level < MIN_PRIORITY || priority.level > MAX_PRIORITY {
                    return Err(ValidationError::PriorityOutOfRange(priority.level));
                }
            }

            // Validate request estimates extension if present
            if let Some(estimates) = &entry.extensions.request_estimates {
                if estimates.estimated_mcycle_count_min > estimates.estimated_mcycle_count_max {
                    return Err(ValidationError::InvalidMcycleRange {
                        min: estimates.estimated_mcycle_count_min,
                        max: estimates.estimated_mcycle_count_max,
                    });
                }
            }
        }

        Ok(())
    }

    /// Parse a requestor list from JSON string
    pub fn from_json(json: &str) -> Result<Self, ValidationError> {
        let list: RequestorList =
            serde_json::from_str(json).map_err(|e| ValidationError::ParseError(e.to_string()))?;
        list.validate()?;
        Ok(list)
    }

    /// Serialize the list to JSON string
    pub fn to_json(&self) -> Result<String, ValidationError> {
        serde_json::to_string_pretty(self).map_err(|e| ValidationError::ParseError(e.to_string()))
    }

    /// Fetch and parse a requestor list from a URL
    pub async fn fetch_from_url(url: &str) -> Result<Self, ValidationError> {
        let response =
            reqwest::get(url).await.map_err(|e| ValidationError::FetchError(e.to_string()))?;

        let json = response.text().await.map_err(|e| ValidationError::FetchError(e.to_string()))?;

        Self::from_json(&json)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_valid_entry() -> RequestorEntry {
        RequestorEntry {
            address: "0xc4ce4f04b9907a9401a0ed7ef073dffebab52aab".parse().unwrap(),
            chain_id: 1,
            name: "Test Requestor".to_string(),
            description: None,
            tags: vec!["test".to_string()],
            extensions: Extensions {
                priority: Some(PriorityExtension { level: 50 }),
                request_estimates: Some(RequestEstimatesExtension {
                    estimated_mcycle_count_min: 100,
                    estimated_mcycle_count_max: 1000,
                    estimated_max_input_size_mb: 10.0,
                }),
                denylist: None,
            },
        }
    }

    #[test]
    fn test_valid_list() {
        let list = RequestorList::new(
            "Test List".to_string(),
            "A test list for validation".to_string(),
            Version { major: 1, minor: 0 },
            vec![create_valid_entry()],
        );
        assert!(list.validate().is_ok());
    }

    #[test]
    fn test_priority_out_of_range() {
        let mut entry = create_valid_entry();
        entry.extensions.priority = Some(PriorityExtension { level: 101 });
        let list = RequestorList::new(
            "Test List".to_string(),
            "A test list for validation".to_string(),
            Version { major: 1, minor: 0 },
            vec![entry],
        );
        assert!(matches!(list.validate(), Err(ValidationError::PriorityOutOfRange(101))));
    }

    #[test]
    fn test_duplicate_address() {
        let entry = create_valid_entry();
        let list = RequestorList::new(
            "Test List".to_string(),
            "A test list for validation".to_string(),
            Version { major: 1, minor: 0 },
            vec![entry.clone(), entry],
        );
        assert!(matches!(list.validate(), Err(ValidationError::DuplicateAddress(_))));
    }

    #[test]
    fn test_invalid_mcycle_range() {
        let mut entry = create_valid_entry();
        entry.extensions.request_estimates = Some(RequestEstimatesExtension {
            estimated_mcycle_count_min: 1000,
            estimated_mcycle_count_max: 100,
            estimated_max_input_size_mb: 10.0,
        });
        let list = RequestorList::new(
            "Test List".to_string(),
            "A test list for validation".to_string(),
            Version { major: 1, minor: 0 },
            vec![entry],
        );
        assert!(matches!(list.validate(), Err(ValidationError::InvalidMcycleRange { .. })));
    }

    #[test]
    fn test_empty_name() {
        let mut entry = create_valid_entry();
        entry.name = "".to_string();
        let list = RequestorList::new(
            "Test List".to_string(),
            "A test list for validation".to_string(),
            Version { major: 1, minor: 0 },
            vec![entry],
        );
        assert!(matches!(list.validate(), Err(ValidationError::EmptyName)));
    }

    #[test]
    fn test_json_serialization() {
        let list = RequestorList::new(
            "Test List".to_string(),
            "A test list for validation".to_string(),
            Version { major: 1, minor: 0 },
            vec![create_valid_entry()],
        );
        let json = list.to_json().unwrap();
        let parsed = RequestorList::from_json(&json).unwrap();
        assert_eq!(list.name, parsed.name);
        assert_eq!(list.description, parsed.description);
        assert_eq!(list.requestors.len(), parsed.requestors.len());
    }

    #[test]
    fn test_parse_standard_example() {
        let json = include_str!("../../../requestor-lists/boundless-priority-list.standard.json");
        let list = RequestorList::from_json(json).unwrap();
        assert_eq!(list.name, "Boundless Recommended Priority List");
        assert!(!list.requestors.is_empty());
        assert!(list.requestors[0].description.is_some());
    }

    #[test]
    fn test_parse_large_example() {
        let json = include_str!("../../../requestor-lists/boundless-priority-list.large.json");
        let list = RequestorList::from_json(json).unwrap();
        assert_eq!(list.name, "Boundless Recommended Priority List for Large Provers");
        assert!(!list.requestors.is_empty());
        assert!(list.requestors[0].description.is_some());
    }

    #[test]
    fn test_parse_allowed_list_example() {
        let json = include_str!("../../../requestor-lists/boundless-allowed-list.json");
        let list = RequestorList::from_json(json).unwrap();
        assert_eq!(list.name, "Boundless Allowed List");
        assert!(!list.requestors.is_empty());
    }

    #[test]
    fn test_multiple_chains_in_list() {
        let entry_chain_1 = RequestorEntry {
            address: "0xc4ce4f04b9907a9401a0ed7ef073dffebab52aab".parse().unwrap(),
            chain_id: 1,
            name: "Mainnet Requestor".to_string(),
            description: None,
            tags: vec![],
            extensions: Extensions {
                priority: Some(PriorityExtension { level: 50 }),
                request_estimates: None,
                denylist: None,
            },
        };

        let entry_chain_8453 = RequestorEntry {
            address: "0xc4ce4f04b9907a9401a0ed7ef073dffebab52aab".parse().unwrap(),
            chain_id: 8453, // Base
            name: "Base Requestor".to_string(),
            description: None,
            tags: vec![],
            extensions: Extensions {
                priority: Some(PriorityExtension { level: 75 }),
                request_estimates: None,
                denylist: None,
            },
        };

        let list = RequestorList::new(
            "Multi-Chain List".to_string(),
            "List with same address on different chains".to_string(),
            Version { major: 1, minor: 0 },
            vec![entry_chain_1, entry_chain_8453],
        );

        assert!(list.validate().is_ok());
        assert_eq!(list.requestors.len(), 2);
    }
}
