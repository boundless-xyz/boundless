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

/// A requestor entry in the priority list
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RequestorEntry {
    /// Ethereum address of the requestor
    pub address: Address,
    /// Chain ID where this requestor operates
    #[serde(rename = "chainId")]
    pub chain_id: u64,
    /// Priority level (0-100, higher = more priority)
    pub priority: i32,
    /// Human-readable name for the requestor
    pub name: String,
    /// Estimated minimum mcycle count for requests from this requestor
    #[serde(rename = "estimatedMcycleCountMin")]
    pub estimated_mcycle_count_min: u64,
    /// Estimated maximum mcycle count for requests from this requestor
    #[serde(rename = "estimatedMcycleCountMax")]
    pub estimated_mcycle_count_max: u64,
    /// Estimated maximum input size in megabytes for requests from this requestor
    #[serde(rename = "estimatedMaxInputSizeMB")]
    pub estimated_max_input_size_mb: f64,
    /// Tags for categorization and filtering
    pub tags: Vec<String>,
}

/// A list of priority requestors
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RequestorList {
    /// Name of this list
    pub name: String,
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
    pub fn new(name: String, version: Version, requestors: Vec<RequestorEntry>) -> Self {
        Self {
            name,
            schema_version: CURRENT_SCHEMA_VERSION,
            version,
            requestors,
        }
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

        let mut seen_addresses = HashSet::new();

        for entry in &self.requestors {
            // Check priority range
            if entry.priority < MIN_PRIORITY || entry.priority > MAX_PRIORITY {
                return Err(ValidationError::PriorityOutOfRange(entry.priority));
            }

            // Check for duplicate addresses
            if !seen_addresses.insert(entry.address) {
                return Err(ValidationError::DuplicateAddress(entry.address));
            }

            // Check mcycle range
            if entry.estimated_mcycle_count_min > entry.estimated_mcycle_count_max {
                return Err(ValidationError::InvalidMcycleRange {
                    min: entry.estimated_mcycle_count_min,
                    max: entry.estimated_mcycle_count_max,
                });
            }

            // Check name is not empty
            if entry.name.trim().is_empty() {
                return Err(ValidationError::EmptyName);
            }
        }

        Ok(())
    }

    /// Parse a requestor list from JSON string
    pub fn from_json(json: &str) -> Result<Self, ValidationError> {
        let list: RequestorList = serde_json::from_str(json)
            .map_err(|e| ValidationError::ParseError(e.to_string()))?;
        list.validate()?;
        Ok(list)
    }

    /// Serialize the list to JSON string
    pub fn to_json(&self) -> Result<String, ValidationError> {
        serde_json::to_string_pretty(self)
            .map_err(|e| ValidationError::ParseError(e.to_string()))
    }

    /// Fetch and parse a requestor list from a URL
    pub async fn fetch_from_url(url: &str) -> Result<Self, ValidationError> {
        let response = reqwest::get(url)
            .await
            .map_err(|e| ValidationError::FetchError(e.to_string()))?;

        let json = response
            .text()
            .await
            .map_err(|e| ValidationError::FetchError(e.to_string()))?;

        Self::from_json(&json)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_valid_entry() -> RequestorEntry {
        RequestorEntry {
            address: "0xc4ce4f04b9907a9401a0ed7ef073dffebab52aab"
                .parse()
                .unwrap(),
            chain_id: 1,
            priority: 50,
            name: "Test Requestor".to_string(),
            estimated_mcycle_count_min: 100,
            estimated_mcycle_count_max: 1000,
            estimated_max_input_size_mb: 10.0,
            tags: vec!["test".to_string()],
        }
    }

    #[test]
    fn test_valid_list() {
        let list = RequestorList::new(
            "Test List".to_string(),
            Version { major: 1, minor: 0 },
            vec![create_valid_entry()],
        );
        assert!(list.validate().is_ok());
    }

    #[test]
    fn test_priority_out_of_range() {
        let mut entry = create_valid_entry();
        entry.priority = 101;
        let list = RequestorList::new(
            "Test List".to_string(),
            Version { major: 1, minor: 0 },
            vec![entry],
        );
        assert!(matches!(
            list.validate(),
            Err(ValidationError::PriorityOutOfRange(101))
        ));
    }

    #[test]
    fn test_duplicate_address() {
        let entry = create_valid_entry();
        let list = RequestorList::new(
            "Test List".to_string(),
            Version { major: 1, minor: 0 },
            vec![entry.clone(), entry],
        );
        assert!(matches!(
            list.validate(),
            Err(ValidationError::DuplicateAddress(_))
        ));
    }

    #[test]
    fn test_invalid_mcycle_range() {
        let mut entry = create_valid_entry();
        entry.estimated_mcycle_count_min = 1000;
        entry.estimated_mcycle_count_max = 100;
        let list = RequestorList::new(
            "Test List".to_string(),
            Version { major: 1, minor: 0 },
            vec![entry],
        );
        assert!(matches!(
            list.validate(),
            Err(ValidationError::InvalidMcycleRange { .. })
        ));
    }

    #[test]
    fn test_empty_name() {
        let mut entry = create_valid_entry();
        entry.name = "".to_string();
        let list = RequestorList::new(
            "Test List".to_string(),
            Version { major: 1, minor: 0 },
            vec![entry],
        );
        assert!(matches!(list.validate(), Err(ValidationError::EmptyName)));
    }

    #[test]
    fn test_json_serialization() {
        let list = RequestorList::new(
            "Test List".to_string(),
            Version { major: 1, minor: 0 },
            vec![create_valid_entry()],
        );
        let json = list.to_json().unwrap();
        let parsed = RequestorList::from_json(&json).unwrap();
        assert_eq!(list.name, parsed.name);
        assert_eq!(list.requestors.len(), parsed.requestors.len());
    }
}
