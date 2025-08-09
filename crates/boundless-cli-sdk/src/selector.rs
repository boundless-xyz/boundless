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

//! Selector utility functions.

use std::collections::HashMap;

use crate::contracts::UNSPECIFIED_SELECTOR;
use crate::util::is_dev_mode;
use alloy_primitives::FixedBytes;
use clap::ValueEnum;
use hex::FromHex;
use risc0_zkvm::sha::Digest;
use std::fmt::{self, Display, Formatter};
use thiserror::Error;

#[derive(Debug, Error)]
#[non_exhaustive]
pub enum SelectorError {
    #[error("Unsupported selector")]
    UnsupportedSelector,
    #[error("Selector {0} does not have verifier parameters")]
    NoVerifierParameters(Selector),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum SelectorType {
    FakeReceipt,
    Groth16,
    SetVerifier,
}

#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Selector {
    FakeReceipt = 0xFFFFFFFF,
    Groth16V1_1 = 0x50bd1769,
    Groth16V1_2 = 0xc101b42b,
    Groth16V2_0 = 0x9f39696c,
    Groth16V2_1 = 0xf536085a,
    Groth16V2_2 = 0xbb001d44,
    SetVerifierV0_1 = 0xbfca9ccb,
    SetVerifierV0_2 = 0x16a15cc8,
    SetVerifierV0_4 = 0xf443ad7b,
    SetVerifierV0_5 = 0xf2e6e6dc,
    SetVerifierV0_6 = 0x80479d24,
    SetVerifierV0_7 = 0x0f63ffd5,
}

impl Display for Selector {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "{:#010x}", *self as u32)
    }
}

impl TryFrom<u32> for Selector {
    type Error = SelectorError;

    fn try_from(value: u32) -> Result<Self, Self::Error> {
        match value {
            0xFFFFFFFF => Ok(Selector::FakeReceipt),
            0x50bd1769 => Ok(Selector::Groth16V1_1),
            0xc101b42b => Ok(Selector::Groth16V1_2),
            0x9f39696c => Ok(Selector::Groth16V2_0),
            0xf536085a => Ok(Selector::Groth16V2_1),
            0xbb001d44 => Ok(Selector::Groth16V2_2),
            0xbfca9ccb => Ok(Selector::SetVerifierV0_1),
            0x16a15cc8 => Ok(Selector::SetVerifierV0_2),
            0xf443ad7b => Ok(Selector::SetVerifierV0_4),
            0xf2e6e6dc => Ok(Selector::SetVerifierV0_5),
            0x80479d24 => Ok(Selector::SetVerifierV0_6),
            0x0f63ffd5 => Ok(Selector::SetVerifierV0_7),
            _ => Err(SelectorError::UnsupportedSelector),
        }
    }
}

impl Selector {
    pub fn verifier_parameters_digest(self) -> Result<Digest, SelectorError> {
        match self {
            Selector::FakeReceipt => {
                Err(SelectorError::NoVerifierParameters(Selector::FakeReceipt))
            }
            Selector::Groth16V1_1 => Ok(Digest::from_hex(
                "50bd1769093e74abda3711c315d84d78e3e282173f6304a33272d92abb590ef5",
            )
            .unwrap()),
            Selector::Groth16V1_2 => Ok(Digest::from_hex(
                "c101b42bcacd62e35222b1207223250814d05dd41d41f8cadc1f16f86707ae15",
            )
            .unwrap()),
            Selector::Groth16V2_0 => Ok(Digest::from_hex(
                "9f39696cb3ae9d6038d6b7a55c09017f0cf35e226ad7582b82dbabb0dae53385",
            )
            .unwrap()),
            Selector::Groth16V2_1 => Ok(Digest::from_hex(
                "f536085a791bdbc6cb46ab3074f88e9e94eabb192de8daca3caee1f4ed811b08",
            )
            .unwrap()),
            Selector::Groth16V2_2 => Ok(Digest::from_hex(
                "bb001d444841d70e8bc0c7d034b349044bf3cf0117afb702b2f1e898b7dd13cc",
            )
            .unwrap()),
            Selector::SetVerifierV0_1 => Ok(Digest::from_hex(
                "bfca9ccb59eb38b8c78ddc399a734d8e0e84e8028b7d616fa54fe707a1ff1b3b",
            )
            .unwrap()),
            Selector::SetVerifierV0_2 => Ok(Digest::from_hex(
                "16a15cc8c94a59dc3e4e41226bc560ecda596a371a487b7ecc6b65d9516dfbdb",
            )
            .unwrap()),
            Selector::SetVerifierV0_4 => Ok(Digest::from_hex(
                "f443ad7bfe538ec90fa38498afd30b27b7d06336f20249b620a6d85ab3c615b6",
            )
            .unwrap()),
            Selector::SetVerifierV0_5 => Ok(Digest::from_hex(
                "f2e6e6dc660ed3ec9d8abb666cd481509c74990fc4d599f3f4a34b9df151f3fd",
            )
            .unwrap()),
            Selector::SetVerifierV0_6 => Ok(Digest::from_hex(
                "80479d24c20613acbaae52f5498cb60f661a26c0681ff2b750611dbaf9ecaa66",
            )
            .unwrap()),
            Selector::SetVerifierV0_7 => Ok(Digest::from_hex(
                "0f63ffd5b1579bf938597f82089ca639a393341e888f58c12d0c91065eb2a3de",
            )
            .unwrap()),
        }
    }

    pub fn get_type(self) -> SelectorType {
        match self {
            Selector::FakeReceipt => SelectorType::FakeReceipt,
            Selector::Groth16V1_1
            | Selector::Groth16V1_2
            | Selector::Groth16V2_0
            | Selector::Groth16V2_1
            | Selector::Groth16V2_2 => SelectorType::Groth16,
            Selector::SetVerifierV0_1
            | Selector::SetVerifierV0_2
            | Selector::SetVerifierV0_4
            | Selector::SetVerifierV0_5
            | Selector::SetVerifierV0_6
            | Selector::SetVerifierV0_7 => SelectorType::SetVerifier,
        }
    }

    pub fn from_bytes(bytes: [u8; 4]) -> Option<Self> {
        Self::try_from(u32::from_be_bytes(bytes)).ok()
    }

    /// Returns the selector corresponding to the Groth16 verifier for the latest zkVM version.
    pub const fn groth16_latest() -> Self {
        Self::Groth16V2_2
    }

    /// Returns the selector corresponding to the latest version of the set inclusion verifier (aka
    /// aggregation verifier).
    pub const fn set_inclusion_latest() -> Self {
        Self::SetVerifierV0_7
    }
}

/// Define the selector types.
///
/// This is used to indicate the type of proof that is being requested.
#[derive(Copy, Clone, Default, Debug, PartialEq, Eq, ValueEnum)]
#[non_exhaustive]
pub enum ProofType {
    /// Any proof type.
    #[default]
    Any,
    /// Groth16 proof type.
    Groth16,
    /// Inclusion proof type.
    Inclusion,
}

/// A struct to hold the supported selectors.
#[derive(Clone, Debug)]
pub struct SupportedSelectors {
    selectors: HashMap<FixedBytes<4>, ProofType>,
}

impl Default for SupportedSelectors {
    fn default() -> Self {
        let mut supported_selectors = Self::new()
            .with_selector(UNSPECIFIED_SELECTOR, ProofType::Any)
            .with_selector(FixedBytes::from(Selector::groth16_latest() as u32), ProofType::Groth16);
        if is_dev_mode() {
            supported_selectors = supported_selectors
                .with_selector(FixedBytes::from(Selector::FakeReceipt as u32), ProofType::Any);
        }
        supported_selectors
    }
}

impl SupportedSelectors {
    /// Create a new `SupportedSelectors` struct.
    pub fn new() -> Self {
        Self { selectors: HashMap::new() }
    }

    /// Add a selector to the supported selectors, taking ownership.
    pub fn with_selector(mut self, selector: FixedBytes<4>, proof_type: ProofType) -> Self {
        self.add_selector(selector, proof_type);
        self
    }

    /// Add a selector to the supported selectors.
    pub fn add_selector(&mut self, selector: FixedBytes<4>, proof_type: ProofType) -> &mut Self {
        self.selectors.insert(selector, proof_type);
        self
    }

    /// Remove a selector from the supported selectors.
    pub fn remove(&mut self, selector: FixedBytes<4>) {
        if self.selectors.contains_key(&selector) {
            self.selectors.remove(&selector);
        }
    }

    /// Check if a selector is supported.
    pub fn is_supported(&self, selector: FixedBytes<4>) -> bool {
        self.selectors.contains_key(&selector)
    }

    /// Check the proof type, returning `None` if unsupported.
    pub fn proof_type(&self, selector: FixedBytes<4>) -> Option<ProofType> {
        self.selectors.get(&selector).cloned()
    }
}

/// Check if a selector is a groth16 selector.
pub fn is_groth16_selector(selector: FixedBytes<4>) -> bool {
    let sel = Selector::from_bytes(selector.into());
    match sel {
        Some(selector) => {
            selector.get_type() == SelectorType::FakeReceipt
                || selector.get_type() == SelectorType::Groth16
        }
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_supported_selectors() {
        let mut supported_selectors = SupportedSelectors::new();
        let selector = FixedBytes::from(Selector::groth16_latest() as u32);
        supported_selectors = supported_selectors.with_selector(selector, ProofType::Groth16);
        assert!(supported_selectors.is_supported(selector));
        supported_selectors.remove(selector);
        assert!(!supported_selectors.is_supported(selector));
    }

    #[test]
    fn test_is_groth16_selector() {
        let selector = FixedBytes::from(Selector::groth16_latest() as u32);
        assert!(is_groth16_selector(selector));
    }
}
