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

//! In-memory snapshot of the `BoundlessRouter` selector registry.
//!
//! The router pairs each verifier *class* with a `requiredAssessorClass`, and a class can hold
//! several entries (concrete impls) at distinct selectors. Resolving a requestor-signed selector to
//! its verifier class — and a verifier class to the assessor class that must seal its fills —
//! requires reading the on-chain registry. [`RouterRegistry`] snapshots exactly the slice of that
//! registry a caller cares about (a fixed set of selectors of interest), then answers resolution
//! queries as pure in-memory lookups with no further RPC. In the future this registry could
//! transparently refresh without interrupting the caller, but for now it is a one-shot snapshot.
//!
//! Build one with
//! [`BoundlessMarketService::load_router_registry`](crate::contracts::boundless_market::BoundlessMarketService::load_router_registry).

use std::collections::HashMap;

use alloy::primitives::{Address, FixedBytes};

use super::UNSPECIFIED_SELECTOR;

/// A registered router entry: the impl a selector pins and the class it belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RouterEntry {
    /// Adapter implementation address registered at the selector.
    pub implementation: Address,
    /// Class this entry belongs to.
    pub class_id: FixedBytes<4>,
    /// Per-call gas cap the router applies when dispatching into the impl.
    pub gas_limit: u64,
}

/// A point-in-time snapshot of the parts of the `BoundlessRouter` registry that matter for a fixed
/// set of selectors of interest. All methods are pure lookups against the snapshot — no RPC.
///
/// The snapshot covers: the chain-default class id, the registered entry for each selector of
/// interest, and the `requiredAssessorClass` of every class referenced by those entries (plus the
/// default class). Selectors / classes outside that slice are treated as absent.
#[derive(Clone, Debug, Default)]
pub struct RouterRegistry {
    default_class_id: FixedBytes<4>,
    /// selector -> entry, for the snapshotted selectors of interest that are registered.
    entries: HashMap<FixedBytes<4>, RouterEntry>,
    /// class id -> `requiredAssessorClass`, for every class referenced by a snapshotted entry or
    /// the default class. `bytes4(0)` means the class declares no required assessor (assessor and
    /// joint classes, or an unset verifier class).
    required_assessor_class: HashMap<FixedBytes<4>, FixedBytes<4>>,
}

impl RouterRegistry {
    /// Assembles a registry from already-fetched parts. Prefer
    /// [`BoundlessMarketService::load_router_registry`](crate::contracts::boundless_market::BoundlessMarketService::load_router_registry),
    /// which fetches these from chain; this constructor exists for that builder and for tests.
    pub fn from_parts(
        default_class_id: FixedBytes<4>,
        entries: HashMap<FixedBytes<4>, RouterEntry>,
        required_assessor_class: HashMap<FixedBytes<4>, FixedBytes<4>>,
    ) -> Self {
        Self { default_class_id, entries, required_assessor_class }
    }

    /// Resolves a requestor-signed verifier selector to its verifier class id, mirroring the three
    /// signed-selector modes the router validates:
    /// - the chain-default sentinel (`0x00000000`) resolves to the default class;
    /// - a registered class id resolves to itself;
    /// - a registered entry selector resolves to its entry's class.
    ///
    /// Returns `None` when the selector is neither a snapshotted entry nor a class this snapshot
    /// covers (i.e. nothing the caller declared interest in), or when the sentinel is signed but no
    /// default class is set.
    pub fn resolve_verifier_class(&self, signed: FixedBytes<4>) -> Option<FixedBytes<4>> {
        if signed == UNSPECIFIED_SELECTOR {
            return (self.default_class_id != FixedBytes::<4>::ZERO)
                .then_some(self.default_class_id);
        }
        if let Some(entry) = self.entries.get(&signed) {
            return Some(entry.class_id);
        }
        // A signed class id is valid only if the snapshot covers it as a class.
        self.required_assessor_class.contains_key(&signed).then_some(signed)
    }

    /// The assessor class a verifier class's fills must be sealed under, or `None` if the class is
    /// not covered by the snapshot or declares no required assessor (assessor / joint classes).
    pub fn required_assessor_class(&self, class_id: FixedBytes<4>) -> Option<FixedBytes<4>> {
        self.required_assessor_class
            .get(&class_id)
            .copied()
            .filter(|class| *class != FixedBytes::<4>::ZERO)
    }

    /// The registered entry for `selector`, if the snapshot covers it.
    pub fn entry(&self, selector: FixedBytes<4>) -> Option<RouterEntry> {
        self.entries.get(&selector).copied()
    }

    /// The chain-default verifier class id (`bytes4(0)` if none is set).
    pub fn default_class_id(&self) -> FixedBytes<4> {
        self.default_class_id
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Mirrors the test-utils router topology: one default verifier class (0x00000010) requiring an
    // assessor class (0x00000020), with two assessor entries and a couple of verifier entries.
    const VERIFIER_CLASS: FixedBytes<4> = FixedBytes([0x00, 0x00, 0x00, 0x10]);
    const ASSESSOR_CLASS: FixedBytes<4> = FixedBytes([0x00, 0x00, 0x00, 0x20]);
    const ONCHAIN_ASSESSOR: FixedBytes<4> = FixedBytes([0x00, 0x00, 0x00, 0x22]);
    const R0_ASSESSOR: FixedBytes<4> = FixedBytes([0x00, 0x00, 0x00, 0x24]);
    const GROTH16: FixedBytes<4> = FixedBytes([0x73, 0xc4, 0x57, 0xba]);

    fn entry(class: FixedBytes<4>, byte: u8) -> RouterEntry {
        RouterEntry { implementation: Address::repeat_byte(byte), class_id: class, gas_limit: 0 }
    }

    fn fixture() -> RouterRegistry {
        let entries = HashMap::from([
            (GROTH16, entry(VERIFIER_CLASS, 0x01)),
            (ONCHAIN_ASSESSOR, entry(ASSESSOR_CLASS, 0x02)),
            (R0_ASSESSOR, entry(ASSESSOR_CLASS, 0x03)),
        ]);
        let required = HashMap::from([
            (VERIFIER_CLASS, ASSESSOR_CLASS),
            (ASSESSOR_CLASS, FixedBytes::<4>::ZERO),
        ]);
        RouterRegistry::from_parts(VERIFIER_CLASS, entries, required)
    }

    #[test]
    fn sentinel_resolves_to_default_class() {
        assert_eq!(fixture().resolve_verifier_class(UNSPECIFIED_SELECTOR), Some(VERIFIER_CLASS));
    }

    #[test]
    fn class_id_resolves_to_itself() {
        assert_eq!(fixture().resolve_verifier_class(VERIFIER_CLASS), Some(VERIFIER_CLASS));
    }

    #[test]
    fn entry_selector_resolves_to_its_class() {
        assert_eq!(fixture().resolve_verifier_class(GROTH16), Some(VERIFIER_CLASS));
    }

    #[test]
    fn unknown_selector_resolves_to_none() {
        assert_eq!(fixture().resolve_verifier_class(FixedBytes([0xde, 0xad, 0xbe, 0xef])), None);
    }

    #[test]
    fn verifier_class_maps_to_required_assessor_class() {
        assert_eq!(fixture().required_assessor_class(VERIFIER_CLASS), Some(ASSESSOR_CLASS));
    }

    #[test]
    fn assessor_class_has_no_required_assessor() {
        assert_eq!(fixture().required_assessor_class(ASSESSOR_CLASS), None);
    }

    #[test]
    fn sentinel_with_no_default_class_is_none() {
        let registry =
            RouterRegistry::from_parts(FixedBytes::<4>::ZERO, HashMap::new(), HashMap::new());
        assert_eq!(registry.resolve_verifier_class(UNSPECIFIED_SELECTOR), None);
    }
}
