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

//! Secret handling and merging utilities.

use crate::display::DisplayManager;

/// Merge an optional value with an existing value, displaying success message if updated
pub fn merge_optional<T: Clone>(
    new_value: &Option<T>,
    existing_value: Option<T>,
    display: &DisplayManager,
    success_message: &str,
) -> Option<T> {
    if let Some(ref value) = new_value {
        display.success(success_message);
        Some(value.clone())
    } else {
        existing_value
    }
}

/// Derive an Ethereum address from a private key
pub fn address_from_private_key(pk: &str) -> Option<alloy::primitives::Address> {
    use alloy::signers::local::PrivateKeySigner;
    pk.parse::<PrivateKeySigner>().ok().map(|signer| signer.address())
}

/// Process a private key: strip 0x prefix and derive address
pub fn process_private_key(pk: &str) -> (String, Option<String>) {
    let pk_clean = pk.strip_prefix("0x").unwrap_or(pk).to_string();
    let address = address_from_private_key(&pk_clean).map(|a| format!("{:#x}", a));
    (pk_clean, address)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_address_from_private_key_valid() {
        let pk = "ac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80";
        let address = address_from_private_key(pk);
        assert!(address.is_some());
        assert_eq!(
            format!("{:#x}", address.unwrap()),
            "0xf39fd6e51aad88f6f4ce6ab8827279cfffb92266"
        );
    }

    #[test]
    fn test_address_from_private_key_with_0x_prefix() {
        let pk = "0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80";
        let address = address_from_private_key(pk);
        assert!(address.is_some());
        assert_eq!(
            format!("{:#x}", address.unwrap()),
            "0xf39fd6e51aad88f6f4ce6ab8827279cfffb92266"
        );
    }

    #[test]
    fn test_address_from_private_key_invalid() {
        let pk = "not_a_valid_key";
        let address = address_from_private_key(pk);
        assert!(address.is_none());
    }

    #[test]
    fn test_merge_optional_with_new_value() {
        let display = DisplayManager::new();
        let new_value = Some("new".to_string());
        let existing_value = Some("old".to_string());

        let result = merge_optional(&new_value, existing_value, &display, "Updated");
        assert_eq!(result, Some("new".to_string()));
    }

    #[test]
    fn test_merge_optional_without_new_value() {
        let display = DisplayManager::new();
        let new_value: Option<String> = None;
        let existing_value = Some("old".to_string());

        let result = merge_optional(&new_value, existing_value, &display, "Updated");
        assert_eq!(result, Some("old".to_string()));
    }

    #[test]
    fn test_merge_optional_both_none() {
        let display = DisplayManager::new();
        let new_value: Option<String> = None;
        let existing_value: Option<String> = None;

        let result = merge_optional(&new_value, existing_value, &display, "Updated");
        assert_eq!(result, None);
    }

    #[test]
    fn test_merge_optional_new_replaces_none() {
        let display = DisplayManager::new();
        let new_value = Some("new".to_string());
        let existing_value: Option<String> = None;

        let result = merge_optional(&new_value, existing_value, &display, "Updated");
        assert_eq!(result, Some("new".to_string()));
    }
}
