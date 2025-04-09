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

use alloy_primitives::FixedBytes;
use clap::ValueEnum;
use risc0_aggregation::SetInclusionReceiptVerifierParameters;
use risc0_ethereum_contracts::selector::{Selector, SelectorType};
use risc0_zkvm::{
    is_dev_mode,
    sha::{Digest, Digestible},
};

use crate::contracts::UNSPECIFIED_SELECTOR;

/// Define the selector types.
///
/// This is used to indicate the type of proof that is being requested.
#[derive(Clone, Debug, PartialEq, Eq, ValueEnum)]
#[non_exhaustive]
pub enum ProofType {
    /// Any proof type.
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
            .with_selector(FixedBytes::from(Selector::Groth16V2_0 as u32), ProofType::Groth16);
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
        let selector = FixedBytes::from(Selector::Groth16V2_0 as u32);
        supported_selectors = supported_selectors.with_selector(selector, ProofType::Groth16);
        assert!(supported_selectors.is_supported(selector));
        supported_selectors.remove(selector);
        assert!(!supported_selectors.is_supported(selector));
    }

    #[test]
    fn test_is_groth16_selector() {
        let selector = FixedBytes::from(Selector::Groth16V2_0 as u32);
        assert!(is_groth16_selector(selector));
    }
}
