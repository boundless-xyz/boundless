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

/// Validate and clean a private key string.
/// Strips whitespace, invisible characters, and optional 0x prefix.
/// Returns the cleaned hex string or an error describing what's wrong.
pub fn validate_private_key(pk: &str) -> Result<String, String> {
    // Strip whitespace, control characters, and non-ASCII invisible chars (e.g. zero-width
    // spaces \u{200B}, BOM \u{FEFF}) that terminals can inject when pasting
    let cleaned: String = pk.chars().filter(|c| c.is_ascii_graphic()).collect();

    // Strip optional 0x prefix
    let hex_str = cleaned.strip_prefix("0x").or_else(|| cleaned.strip_prefix("0X")).unwrap_or(&cleaned);

    if hex_str.is_empty() {
        return Err("Private key is empty".to_string());
    }

    // Check for non-hex characters
    if let Some(pos) = hex_str.find(|c: char| !c.is_ascii_hexdigit()) {
        let bad_char = hex_str.chars().nth(pos).unwrap();
        return Err(format!(
            "Private key contains invalid character '{}' at position {}. Only hex characters (0-9, a-f, A-F) are allowed",
            bad_char, pos
        ));
    }

    // Check length (should be exactly 64 hex chars = 32 bytes)
    if hex_str.len() != 64 {
        return Err(format!(
            "Private key has {} hex characters, expected 64 (32 bytes)",
            hex_str.len()
        ));
    }

    // Verify it actually parses as a valid private key
    let lowercase = hex_str.to_lowercase();
    if address_from_private_key(&lowercase).is_none() {
        return Err("Private key is not a valid secp256k1 private key".to_string());
    }

    Ok(lowercase)
}

/// Process a private key: strip 0x prefix, validate, and derive address.
/// Returns an error if the key is invalid.
pub fn process_private_key(pk: &str) -> Result<(String, Option<String>), String> {
    let pk_clean = validate_private_key(pk)?;
    let address = address_from_private_key(&pk_clean).map(|a| format!("{:#x}", a));
    Ok((pk_clean, address))
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

    // ─── validate_private_key tests ─────────────────────────────────────

    const VALID_PK: &str = "ac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80";
    const VALID_PK_ADDRESS: &str = "0xf39fd6e51aad88f6f4ce6ab8827279cfffb92266";

    #[test]
    fn test_validate_private_key_valid_bare() {
        let result = validate_private_key(VALID_PK).unwrap();
        assert_eq!(result, VALID_PK);
    }

    #[test]
    fn test_validate_private_key_with_0x_lowercase() {
        let pk = format!("0x{}", VALID_PK);
        let result = validate_private_key(&pk).unwrap();
        assert_eq!(result, VALID_PK);
    }

    #[test]
    fn test_validate_private_key_with_0x_uppercase() {
        let pk = format!("0X{}", VALID_PK);
        let result = validate_private_key(&pk).unwrap();
        assert_eq!(result, VALID_PK);
    }

    #[test]
    fn test_validate_private_key_mixed_case_hex() {
        let pk = "AC0974BEC39A17E36BA4A6B4D238FF944BACB478CBED5EFCAE784D7BF4F2FF80";
        let result = validate_private_key(pk).unwrap();
        assert_eq!(result, VALID_PK); // should be lowercased
    }

    #[test]
    fn test_validate_private_key_with_leading_trailing_whitespace() {
        let pk = format!("  0x{}  \n", VALID_PK);
        let result = validate_private_key(&pk).unwrap();
        assert_eq!(result, VALID_PK);
    }

    #[test]
    fn test_validate_private_key_with_tabs_and_carriage_return() {
        let pk = format!("\t0x{}\r\n", VALID_PK);
        let result = validate_private_key(&pk).unwrap();
        assert_eq!(result, VALID_PK);
    }

    #[test]
    fn test_validate_private_key_with_embedded_spaces() {
        // Some terminals insert spaces when pasting
        let pk = "ac09 74be c39a 17e3 6ba4 a6b4 d238 ff94 4bac b478 cbed 5efc ae78 4d7b f4f2 ff80";
        let result = validate_private_key(pk).unwrap();
        assert_eq!(result, VALID_PK);
    }

    #[test]
    fn test_validate_private_key_with_control_chars() {
        // Zero-width spaces and other invisible chars from copy-paste
        let pk = format!("0x{}\u{200B}\u{FEFF}", VALID_PK);
        let result = validate_private_key(&pk).unwrap();
        assert_eq!(result, VALID_PK);
    }

    #[test]
    fn test_validate_private_key_empty() {
        let err = validate_private_key("").unwrap_err();
        assert!(err.contains("empty"));
    }

    #[test]
    fn test_validate_private_key_only_whitespace() {
        let err = validate_private_key("   \n\t  ").unwrap_err();
        assert!(err.contains("empty"));
    }

    #[test]
    fn test_validate_private_key_only_0x_prefix() {
        let err = validate_private_key("0x").unwrap_err();
        assert!(err.contains("empty"));
    }

    #[test]
    fn test_validate_private_key_too_short() {
        let pk = "ac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff8";
        let err = validate_private_key(pk).unwrap_err();
        assert!(err.contains("63 hex characters, expected 64"));
    }

    #[test]
    fn test_validate_private_key_too_long() {
        let pk = format!("{}00", VALID_PK); // 66 hex chars
        let err = validate_private_key(&pk).unwrap_err();
        assert!(err.contains("66 hex characters, expected 64"));
    }

    #[test]
    fn test_validate_private_key_invalid_char_at_start() {
        let pk = "gc0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80";
        let err = validate_private_key(pk).unwrap_err();
        assert!(err.contains("invalid character 'g' at position 0"));
    }

    #[test]
    fn test_validate_private_key_invalid_char_in_middle() {
        let pk = "ac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4fZff80";
        let err = validate_private_key(pk).unwrap_err();
        assert!(err.contains("invalid character 'Z'"));
    }

    #[test]
    fn test_validate_private_key_all_zeros() {
        // All zeros is not a valid secp256k1 private key
        let pk = "0000000000000000000000000000000000000000000000000000000000000000";
        let err = validate_private_key(pk).unwrap_err();
        assert!(err.contains("not a valid secp256k1 private key"));
    }

    #[test]
    fn test_validate_private_key_returns_lowercase() {
        let pk = "AC0974BEC39A17E36BA4A6B4D238FF944BACB478CBED5EFCAE784D7BF4F2FF80";
        let result = validate_private_key(pk).unwrap();
        assert!(result.chars().all(|c| !c.is_ascii_uppercase()));
    }

    // ─── process_private_key tests ──────────────────────────────────────

    #[test]
    fn test_process_private_key_valid_returns_clean_key_and_address() {
        let pk = format!("0x{}", VALID_PK);
        let (clean, addr) = process_private_key(&pk).unwrap();
        assert_eq!(clean, VALID_PK);
        assert_eq!(addr.unwrap(), VALID_PK_ADDRESS);
    }

    #[test]
    fn test_process_private_key_with_whitespace() {
        let pk = format!("  0x{}  \n", VALID_PK);
        let (clean, addr) = process_private_key(&pk).unwrap();
        assert_eq!(clean, VALID_PK);
        assert!(addr.is_some());
    }

    #[test]
    fn test_process_private_key_invalid_returns_error() {
        assert!(process_private_key("not_valid").is_err());
        assert!(process_private_key("").is_err());
        assert!(process_private_key("0x").is_err());
        assert!(process_private_key("0xZZZZ").is_err());
    }

    #[test]
    fn test_process_private_key_error_is_descriptive() {
        let err = process_private_key("tooshort").unwrap_err();
        assert!(err.contains("hex characters, expected 64") || err.contains("invalid character"));
    }
}
