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

use alloy_primitives::FixedBytes;
use serde::{Deserialize, Serialize};

use crate::request::UNSPECIFIED_SELECTOR;

/// Define the selector types.
///
/// This is used to indicate the type of proof that is being requested.
#[derive(Copy, Clone, Default, Debug, PartialEq, Eq, Serialize, Deserialize)]
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

#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub(crate) enum Selector {
    FakeReceipt = 0xFFFFFFFF,
    Groth16V2_2 = 0xbb001d44,
    SetVerifierV0_7 = 0x0f63ffd5,
}
impl Selector {
    /// Returns the latest Set Inclusion selector.
    pub(crate) fn set_inclusion_latest() -> FixedBytes<4> {
        FixedBytes::from(Selector::SetVerifierV0_7 as u32)
    }

    /// Returns the latest Groth16 selector.
    pub(crate) fn groth16_latest() -> FixedBytes<4> {
        FixedBytes::from(Selector::Groth16V2_2 as u32)
    }
    pub(crate) fn fake_receipt() -> FixedBytes<4> {
        FixedBytes::from(Selector::FakeReceipt as u32)
    }
}

pub(crate) fn proof_type(selector: FixedBytes<4>) -> Option<ProofType> {
    match selector {
        s if s.0 == UNSPECIFIED_SELECTOR.0 => Some(ProofType::Any),
        s if s.0 == Selector::groth16_latest().0 => Some(ProofType::Groth16),
        s if s.0 == Selector::set_inclusion_latest().0 => Some(ProofType::Inclusion),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use risc0_ethereum_contracts::selector::Selector as OfficialSelector;

    #[test]
    fn test_selector_consistency() {
        assert_eq!(
            Selector::groth16_latest().0,
            FixedBytes::from(OfficialSelector::groth16_latest() as u32).0
        );
        assert_eq!(
            Selector::set_inclusion_latest().0,
            FixedBytes::from(OfficialSelector::set_inclusion_latest() as u32).0
        );
    }
}
