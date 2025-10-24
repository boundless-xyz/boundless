// Copyright 2025 RISC Zero, Inc.
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

/// Obscure a secret for display (show first 3 and last 3 characters)
pub fn obscure_secret(secret: &str) -> String {
    if secret.len() <= 8 {
        "****".to_string()
    } else {
        format!("{}...{}", &secret[..3], &secret[secret.len() - 3..])
    }
}

/// Obscure a single segment (between dots or slashes)
fn obscure_segment(segment: &str) -> String {
    let len = segment.len();
    if len <= 6 {
        // Too short to obscure meaningfully
        segment.to_string()
    } else if len <= 10 {
        // Show 2 chars + *** + 2 chars
        format!("{}***{}", segment[..2].to_lowercase(), segment[len - 2..].to_lowercase())
    } else {
        // Show 3-4 chars + ***** + 3-4 chars (scale with length)
        let show_chars = if len > 15 { 4 } else { 3 };
        let stars = if len > 15 { "*****" } else { "***" };
        format!(
            "{}{}{}",
            segment[..show_chars].to_lowercase(),
            stars,
            segment[len - show_chars..].to_lowercase()
        )
    }
}

/// Obscure a URL for display (obscure segments between dots and slashes)
pub fn obscure_url(url: &str) -> String {
    // Split by :// to separate protocol
    if let Some((protocol, rest)) = url.split_once("://") {
        let parts = rest.split('/').collect::<Vec<_>>();

        if parts.is_empty() {
            return url.to_string();
        }

        // Process the host (first part) - split by dots
        let host = parts[0];
        let obscured_host =
            host.split('.').map(obscure_segment).collect::<Vec<_>>().join(".");

        // Process the path segments
        let obscured_path: Vec<String> =
            parts[1..].iter().map(|&segment| obscure_segment(segment)).collect();

        // Reconstruct URL
        if obscured_path.is_empty() {
            format!("{}://{}", protocol, obscured_host)
        } else {
            format!("{}://{}/{}", protocol, obscured_host, obscured_path.join("/"))
        }
    } else {
        // No protocol, just obscure each slash-separated part
        url.split('/').map(obscure_segment).collect::<Vec<_>>().join("/")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_obscure_secret_short() {
        assert_eq!(obscure_secret("abc"), "****");
        assert_eq!(obscure_secret("12345678"), "****");
    }

    #[test]
    fn test_obscure_secret_long() {
        assert_eq!(obscure_secret("abcdefghijk"), "abc...ijk");
        assert_eq!(obscure_secret("0x1234567890abcdef"), "0x1...def");
    }

    #[test]
    fn test_obscure_url_simple() {
        let url = "https://example.com/api/v1";
        let obscured = obscure_url(url);
        // "example" (7 chars) -> "ex***le", "com" stays, "api" and "v1" stay
        assert_eq!(obscured, "https://ex***le.com/api/v1");
    }

    #[test]
    fn test_obscure_url_with_api_key() {
        let url = "https://eth-mainnet.g.alchemy.com/v2/kEepgHsajdisoajJcfV";
        let obscured = obscure_url(url);
        assert_eq!(obscured, "https://eth***net.g.al***my.com/v2/keep*****jcfv");
    }

    #[test]
    fn test_obscure_url_long_segments() {
        let url = "https://verylongsubdomain.anotherlongdomain.com/verylongpath/anotherlongpath";
        let obscured = obscure_url(url);
        assert_eq!(obscured, "https://very*****main.anot*****main.com/ver***ath/ano***ath");
    }

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
