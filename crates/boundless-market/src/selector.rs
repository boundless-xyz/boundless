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

//! Selector utility functions.

use std::collections::HashMap;
use std::fmt::{self, Display, Formatter};

use alloy_primitives::FixedBytes;
use clap::ValueEnum;
use risc0_aggregation::SetInclusionReceiptVerifierParameters;
use risc0_ethereum_contracts::selector::Selector;
use risc0_zkvm::sha::{Digest, Digestible};
use thiserror::Error;

use crate::contracts::UNSPECIFIED_SELECTOR;
use crate::util::is_dev_mode;

#[derive(Debug, Error)]
#[non_exhaustive]
/// Errors related to extended selectors.
pub enum SelectorExtError {
    /// Unsupported selector error.
    #[error("Unsupported selector")]
    UnsupportedSelector,
    /// No verifier parameters error.
    #[error("Selector {0} does not have verifier parameters")]
    NoVerifierParameters(SelectorExt),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
/// Types of extended selectors.
pub enum SelectorExtType {
    /// A fake receipt selector used for testing and development.
    FakeReceipt,
    /// Groth16 proof selector.
    Groth16,
    /// Set verifier selector.
    SetVerifier,
    /// Blake3 Groth16 selector.
    Blake3Groth16,
    /// Fake Blake3 Groth16 selector
    FakeBlake3Groth16,
}

#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
/// Extended selectors for various proof types.
pub enum SelectorExt {
    /// A fake receipt selector used for testing and development.
    FakeReceipt = 0xFFFFFFFF,
    /// A fake Blake3 Groth16 proof selector.
    FakeBlake3Groth16 = 0xFFFF0000,
    /// Groth16 proof selector version 3.0.
    Groth16V3_0 = Selector::Groth16V3_0 as u32,
    /// Set verifier selector version 0.9.
    SetVerifierV0_9 = Selector::SetVerifierV0_9 as u32,
    /// Blake3 Groth16 selector version 0.1.
    Blake3Groth16V0_1 = 0x62f049f6,
}

impl Display for SelectorExt {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "{:#010x}", *self as u32)
    }
}

impl TryFrom<u32> for SelectorExt {
    type Error = SelectorExtError;

    fn try_from(value: u32) -> Result<Self, Self::Error> {
        match value {
            0xFFFFFFFF => Ok(SelectorExt::FakeReceipt),
            0xFFFF0000 => Ok(SelectorExt::FakeBlake3Groth16),
            0x73c457ba => Ok(SelectorExt::Groth16V3_0),
            0x242f9d5b => Ok(SelectorExt::SetVerifierV0_9),
            0x62f049f6 => Ok(SelectorExt::Blake3Groth16V0_1),
            _ => Err(SelectorExtError::UnsupportedSelector),
        }
    }
}

impl TryFrom<Selector> for SelectorExt {
    type Error = SelectorExtError;

    fn try_from(value: Selector) -> Result<Self, Self::Error> {
        Self::try_from(value as u32)
    }
}

impl SelectorExt {
    /// Get the latest groth16 selector.
    pub fn groth16_latest() -> SelectorExt {
        SelectorExt::Groth16V3_0
    }

    /// Get the latest set verifier selector.
    pub fn set_inclusion_latest() -> SelectorExt {
        SelectorExt::SetVerifierV0_9
    }

    /// Get the latest blake3 groth16 selector.
    pub fn blake3_groth16_latest() -> SelectorExt {
        // Currently, we only have one blake3 groth16 selector.
        SelectorExt::Blake3Groth16V0_1
    }

    /// Create a `SelectorExt` from a 4-byte array.
    pub fn from_bytes(bytes: [u8; 4]) -> Option<Self> {
        Self::try_from(u32::from_be_bytes(bytes)).ok()
    }

    /// Returns the type of the selector.
    pub fn get_type(self) -> SelectorExtType {
        match self {
            SelectorExt::FakeReceipt => SelectorExtType::FakeReceipt,
            SelectorExt::FakeBlake3Groth16 => SelectorExtType::FakeBlake3Groth16,
            SelectorExt::Groth16V3_0 => SelectorExtType::Groth16,
            SelectorExt::SetVerifierV0_9 => SelectorExtType::SetVerifier,
            SelectorExt::Blake3Groth16V0_1 => SelectorExtType::Blake3Groth16,
        }
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
    /// BitVM compatible blake3 Groth16 proof type.
    Blake3Groth16,
}

/// A struct to hold the supported selectors.
#[derive(Clone, Debug)]
pub struct SupportedSelectors {
    /// A map of selectors to their proof type.
    pub selectors: HashMap<FixedBytes<4>, ProofType>,
}

impl Default for SupportedSelectors {
    fn default() -> Self {
        let mut supported_selectors = Self::new()
            .with_selector(UNSPECIFIED_SELECTOR, ProofType::Any)
            .with_selector(
                FixedBytes::from(SelectorExt::groth16_latest() as u32),
                ProofType::Groth16,
            )
            .with_selector(
                FixedBytes::from(SelectorExt::blake3_groth16_latest() as u32),
                ProofType::Blake3Groth16,
            );
        if is_dev_mode() {
            supported_selectors = supported_selectors
                .with_selector(FixedBytes::from(SelectorExt::FakeReceipt as u32), ProofType::Any)
                .with_selector(
                    FixedBytes::from(SelectorExt::FakeBlake3Groth16 as u32),
                    ProofType::Blake3Groth16,
                )
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

    /// Add a selector calculated from the given set builder image ID.
    ///
    /// The selector is calculated by constructing the [SetInclusionReceiptVerifierParameters]
    /// using the given image ID. The resulting selector has [ProofType::Inclusion].
    pub fn with_set_builder_image_id(&self, set_builder_image_id: impl Into<Digest>) -> Self {
        let verifier_params =
            SetInclusionReceiptVerifierParameters { image_id: set_builder_image_id.into() }
                .digest();
        let set_builder_selector: FixedBytes<4> =
            verifier_params.as_bytes()[0..4].try_into().unwrap();
        let mut selectors = self.selectors.clone();
        selectors.insert(set_builder_selector, ProofType::Inclusion);

        Self { selectors }
    }
}

/// Check if a selector is a groth16 selector.
pub fn is_groth16_selector(selector: FixedBytes<4>) -> bool {
    let sel = SelectorExt::from_bytes(selector.into());
    match sel {
        Some(selector) => {
            selector.get_type() == SelectorExtType::FakeReceipt
                || selector.get_type() == SelectorExtType::Groth16
        }
        None => false,
    }
}

/// Check if a selector is a blake3 groth16 selector.
pub fn is_blake3_groth16_selector(selector: FixedBytes<4>) -> bool {
    let sel = SelectorExt::from_bytes(selector.into());
    match sel {
        Some(selector) => {
            selector.get_type() == SelectorExtType::FakeBlake3Groth16
                || selector.get_type() == SelectorExtType::Blake3Groth16
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
        let selector = FixedBytes::from(SelectorExt::groth16_latest() as u32);
        supported_selectors = supported_selectors.with_selector(selector, ProofType::Groth16);
        assert!(supported_selectors.is_supported(selector));
        supported_selectors.remove(selector);
        assert!(!supported_selectors.is_supported(selector));
    }

    #[test]
    fn test_is_groth16_selector() {
        let selector = FixedBytes::from(SelectorExt::groth16_latest() as u32);
        assert!(is_groth16_selector(selector));
        let fake_selector = FixedBytes::from(SelectorExt::FakeReceipt as u32);
        assert!(is_groth16_selector(fake_selector));
    }

    #[test]
    fn test_is_blake3_groth16_selector() {
        let selector = FixedBytes::from(SelectorExt::blake3_groth16_latest() as u32);
        assert!(is_blake3_groth16_selector(selector));
        let fake_selector = FixedBytes::from(SelectorExt::FakeBlake3Groth16 as u32);
        assert!(is_blake3_groth16_selector(fake_selector));
    }
}
