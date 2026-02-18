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

//! BIP-39/BIP-32 key derivation for address rotation.
//!
//! Derives keys using the standard Ethereum path: m/44'/60'/0'/0/{index}.
//! Index 0 = funding key, indices 1..N = rotation keys.
//! Keys match MetaMask, Ledger, and other standard wallets.

use alloy::signers::local::{coins_bip39::English, MnemonicBuilder, PrivateKeySigner};

/// Derive `count` keys from a BIP-39 mnemonic phrase.
///
/// Keys are derived at m/44'/60'/0'/0/0, m/44'/60'/0'/0/1, ..., m/44'/60'/0'/0/(count-1).
/// Index 0 is the funding key; indices 1..count are rotation keys.
pub fn derive_keys(phrase: &str, count: usize) -> anyhow::Result<Vec<PrivateKeySigner>> {
    if count == 0 {
        return Ok(Vec::new());
    }
    let mut signers = Vec::with_capacity(count);
    for i in 0..count {
        let signer = MnemonicBuilder::<English>::default()
            .phrase(phrase.trim())
            .index(i as u32)
            .map_err(|e| anyhow::anyhow!("MnemonicBuilder index {i}: {e}"))?
            .build()
            .map_err(|e| anyhow::anyhow!("MnemonicBuilder build index {i}: {e}"))?;
        signers.push(signer);
    }
    Ok(signers)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_derive_deterministic() {
        let phrase = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";
        let a = derive_keys(phrase, 3).unwrap();
        let b = derive_keys(phrase, 3).unwrap();
        assert_eq!(a.len(), 3);
        assert_eq!(b.len(), 3);
        for i in 0..3 {
            assert_eq!(a[i].address(), b[i].address(), "index {i} should be deterministic");
        }
    }

    #[test]
    fn test_derive_different_indices() {
        let phrase = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";
        let keys = derive_keys(phrase, 3).unwrap();
        let addrs: Vec<_> = keys.iter().map(|k| k.address()).collect();
        assert_ne!(addrs[0], addrs[1]);
        assert_ne!(addrs[1], addrs[2]);
        assert_ne!(addrs[0], addrs[2]);
    }

    #[test]
    fn test_derive_empty() {
        let phrase = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";
        let keys = derive_keys(phrase, 0).unwrap();
        assert!(keys.is_empty());
    }
}
