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
use risc0_aggregation::SetInclusionReceiptVerifierParameters;
use risc0_ethereum_contracts::selector::{Selector, SelectorType};
use risc0_zkvm::{
    is_dev_mode,
    sha::{Digest, Digestible},
};

use crate::contracts::UNSPECIFIED_SELECTOR;

/// A struct to hold the supported selectors.
#[derive(Clone, Debug)]
pub struct SupportedSelectors {
    selectors: HashMap<FixedBytes<4>, bool>,
}

impl Default for SupportedSelectors {
    fn default() -> Self {
        let mut supported_selectors = Self::new()
            .with_selector(UNSPECIFIED_SELECTOR)
            .with_selector(FixedBytes::from(Selector::Groth16V1_2 as u32));
        if is_dev_mode() {
            supported_selectors =
                supported_selectors.with_selector(FixedBytes::from(Selector::FakeReceipt as u32));
        }
        supported_selectors
    }
}

impl SupportedSelectors {
    /// Create a new `SupportedSelectors` struct.
    pub fn new() -> Self {
        Self { selectors: HashMap::new() }
    }

    /// Add a selector to the supported selectors.
    pub fn with_selector(&self, selector: FixedBytes<4>) -> Self {
        let mut selectors = self.selectors.clone();
        selectors.insert(selector, true);
        Self { selectors }
    }

    /// Remove a selector from the supported selectors.
    pub fn remove(&mut self, selector: FixedBytes<4>) {
        if self.selectors.contains_key(&selector) {
            self.selectors.remove(&selector);
        }
    }
    /// Check if a selector is supported.
    pub fn is_supported(&self, selector: &FixedBytes<4>) -> bool {
        self.selectors.contains_key(selector)
    }

    /// Set the set builder image ID.
    pub fn with_set_builder_image_id(&self, set_builder_image_id: impl Into<Digest>) -> Self {
        let verifier_params =
            SetInclusionReceiptVerifierParameters { image_id: set_builder_image_id.into() }
                .digest();
        let set_builder_selector: FixedBytes<4> =
            verifier_params.as_bytes()[0..4].try_into().unwrap();
        let mut selectors = self.selectors.clone();
        selectors.insert(set_builder_selector, true);

        Self { selectors }
    }
}

/// Check if a selector is an unaggregated selector.
pub fn is_unaggregated_selector(selector: FixedBytes<4>) -> bool {
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
        let selector = FixedBytes::from(Selector::Groth16V1_2 as u32);
        supported_selectors = supported_selectors.with_selector(selector);
        assert!(supported_selectors.is_supported(&selector));
        supported_selectors.remove(selector);
        assert!(!supported_selectors.is_supported(&selector));
    }

    #[test]
    fn test_is_unaggregated_selector() {
        let selector = FixedBytes::from(Selector::Groth16V1_2 as u32);
        assert!(is_unaggregated_selector(selector));
    }
}
