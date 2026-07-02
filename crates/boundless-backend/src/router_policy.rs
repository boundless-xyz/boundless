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

//! Broker-policy view over a `BoundlessRouter` registry snapshot.

use std::collections::HashMap;
use std::sync::Arc;

use alloy::primitives::FixedBytes;
use boundless_market::contracts::{RouterEntry, RouterRegistry, UNSPECIFIED_SELECTOR};

/// A [`RouterRegistry`] snapshot combined with one backend's capability inputs — which verifier
/// selectors that backend can produce and which assessor selectors it can satisfy, in priority
/// order — resolved into the policy questions asked per order and per batch: which verifier
/// selector to emit for a signed selector, which assessor seals a batch, and which selectors /
/// classes are supported at all. All methods are pure in-memory lookups.
///
/// The registry snapshot is global chain state; the producible selectors and assessor priority
/// are this backend's capabilities. The policy is therefore per-backend, but it holds the registry
/// behind an [`Arc`] so the one global snapshot can be shared across backends.
#[derive(Clone, Debug)]
pub struct RouterPolicy {
    registry: Arc<RouterRegistry>,
    /// Candidate assessor selectors in descending priority; the first registered in a verifier
    /// class's `requiredAssessorClass` wins.
    assessor_priority: Vec<FixedBytes<4>>,
    /// Registry-covered producible verifier entry selector -> the selector the broker emits in a
    /// seal for it. A requestor may pin any of these keys as an exact-version requirement; the
    /// set-inclusion entry (guest-version-derived, not a compile-time constant) is the one this
    /// captures that a hardcoded list cannot.
    producible: HashMap<FixedBytes<4>, FixedBytes<4>>,
    /// Verifier class id -> the emitted selector for the backend's *most-preferred* producible
    /// entry in that class. A class is a proof-type version family and may hold several producible
    /// entries; signing the class id means "any version", and this resolves to the one the backend
    /// prefers (first in the `producible_entries` preference order — see [`Self::new`]).
    class_to_producible: HashMap<FixedBytes<4>, FixedBytes<4>>,
    /// Verifier classes the broker fully supports: it can produce a verifier entry in the class
    /// and a candidate assessor is registered in the class's `requiredAssessorClass`.
    supported_classes: Vec<FixedBytes<4>>,
}

impl RouterPolicy {
    /// Derives the policy from a registry snapshot and one backend's capabilities.
    /// `producible_entries` pairs each verifier selector that backend can serve with the selector
    /// it emits in a seal, **in descending preference**; pairs the registry does not cover are
    /// dropped. When a class holds several producible entries (a proof-type version family the
    /// backend can serve more than one version of), the *first* pair for that class wins
    /// [`Self::producible_selector`] — i.e. the backend's declared preference, deterministically,
    /// independent of map iteration order. `assessor_priority` lists the candidate assessor
    /// selectors in descending preference.
    pub fn new(
        registry: Arc<RouterRegistry>,
        producible_entries: Vec<(FixedBytes<4>, FixedBytes<4>)>,
        assessor_priority: Vec<FixedBytes<4>>,
    ) -> Self {
        let mut producible = HashMap::new();
        let mut class_to_producible = HashMap::new();
        // `producible_entries` is in descending preference, so `or_insert` keeps the first
        // (preferred) entry registered for each class and ignores lower-preference siblings.
        for (selector, produced) in producible_entries {
            if let Some(entry) = registry.entry(selector) {
                producible.insert(selector, produced);
                class_to_producible.entry(entry.class_id).or_insert(produced);
            }
        }
        let mut policy = Self {
            registry,
            assessor_priority,
            producible,
            class_to_producible,
            supported_classes: Vec::new(),
        };
        policy.supported_classes = policy
            .class_to_producible
            .keys()
            .copied()
            .filter(|class| policy.assessor_for_verifier_class(*class).is_some())
            .collect();
        policy
    }

    /// The verifier selector the broker emits for orders signed against `signed`: a producible
    /// entry selector maps to its emitted selector (e.g. the set-inclusion entry emits the
    /// unspecified-selector seal), a verifier class id maps to the broker's preferred producible
    /// entry in that class, and anything else — notably the default sentinel — is `None` (passes
    /// through unmapped).
    pub fn producible_selector(&self, signed: FixedBytes<4>) -> Option<FixedBytes<4>> {
        self.producible.get(&signed).or_else(|| self.class_to_producible.get(&signed)).copied()
    }

    /// The registry-covered verifier entry selectors the broker can produce — the exact-version
    /// pins a requestor may sign, advertised through the backend's supported selectors. Includes
    /// the guest-version-derived set-inclusion selector, which no compile-time list can name.
    pub fn producible_selectors(&self) -> impl Iterator<Item = FixedBytes<4>> + '_ {
        self.producible.keys().copied()
    }

    /// The verifier classes the broker fully supports (producible verifier entry present AND a
    /// candidate assessor registered in the class's `requiredAssessorClass`).
    pub fn supported_classes(&self) -> &[FixedBytes<4>] {
        &self.supported_classes
    }

    /// Every selector a requestor may sign that this backend can produce AND seal: the producible
    /// verifier entry selectors (exact-version pins) whose class has a reachable assessor, the
    /// fully-supported verifier class ids ("any version"), and the chain-default sentinel.
    ///
    /// The sentinel (`0x00000000`) is an alias for the default class — never a class id itself
    /// (the router reserves it) — so it is serveable exactly when that class is fully supported,
    /// i.e. is in [`Self::supported_classes`].
    pub fn supported_selectors(&self) -> Vec<FixedBytes<4>> {
        let mut selectors: Vec<FixedBytes<4>> = self
            .producible_selectors()
            .filter(|sel| self.assessor_selector_for_signed(*sel).is_some())
            .collect();
        selectors.extend(self.supported_classes.iter().copied());
        if self.supported_classes.contains(&self.registry.default_class_id()) {
            selectors.push(UNSPECIFIED_SELECTOR);
        }
        selectors
    }

    /// The registered router entry for `selector`, if the snapshot covers it. Used e.g. to look
    /// up the on-chain assessor adapter address behind its selector.
    pub fn entry(&self, selector: FixedBytes<4>) -> Option<RouterEntry> {
        self.registry.entry(selector)
    }

    /// The assessor selector that would seal a batch containing an order with this signed
    /// verifier selector (resolve the verifier class, then the highest-priority candidate
    /// assessor in its `requiredAssessorClass`). Orders sharing this value may share a batch —
    /// it is both the batch grouping key and what the seal is built with.
    pub fn assessor_selector_for_signed(&self, signed: FixedBytes<4>) -> Option<FixedBytes<4>> {
        let verifier_class = self.registry.resolve_verifier_class(signed)?;
        self.assessor_for_verifier_class(verifier_class)
    }

    /// The highest-priority candidate assessor selector registered in `verifier_class`'s
    /// `requiredAssessorClass`, or `None` if the class has no required assessor or none of the
    /// candidates are registered there.
    fn assessor_for_verifier_class(&self, verifier_class: FixedBytes<4>) -> Option<FixedBytes<4>> {
        let assessor_class = self.registry.required_assessor_class(verifier_class)?;
        self.assessor_priority
            .iter()
            .copied()
            .find(|&sel| self.registry.entry(sel).is_some_and(|e| e.class_id == assessor_class))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::primitives::Address;

    const V_CLASS: FixedBytes<4> = FixedBytes([0xAA, 0x00, 0x00, 0x01]);
    const A_CLASS: FixedBytes<4> = FixedBytes([0xAA, 0x00, 0x00, 0x02]);
    const V_A: FixedBytes<4> = FixedBytes([0x00, 0x00, 0x00, 0x11]);
    const V_B: FixedBytes<4> = FixedBytes([0x00, 0x00, 0x00, 0x12]);
    const ASSESSOR: FixedBytes<4> = FixedBytes([0x00, 0x00, 0x00, 0x22]);

    fn entry(class: FixedBytes<4>) -> RouterEntry {
        RouterEntry { implementation: Address::ZERO, class_id: class, gas_limit: 0 }
    }

    /// One verifier class (`V_CLASS`) holding TWO producible entries (`V_A`, `V_B`) — a proof-type
    /// version family the backend can serve more than one version of — plus an assessor entry.
    fn registry() -> Arc<RouterRegistry> {
        let entries = HashMap::from([
            (V_A, entry(V_CLASS)),
            (V_B, entry(V_CLASS)),
            (ASSESSOR, entry(A_CLASS)),
        ]);
        let required = HashMap::from([(V_CLASS, A_CLASS), (A_CLASS, FixedBytes::<4>::ZERO)]);
        Arc::new(RouterRegistry::from_parts(V_CLASS, entries, required))
    }

    #[test]
    fn class_id_resolves_to_preferred_producible_entry() {
        // V_A listed first => higher preference.
        let policy = RouterPolicy::new(registry(), vec![(V_A, V_A), (V_B, V_B)], vec![ASSESSOR]);
        // Signing the class id picks the preferred (first-declared) entry.
        assert_eq!(policy.producible_selector(V_CLASS), Some(V_A));
        // Pinning a specific entry selector still resolves to that exact version.
        assert_eq!(policy.producible_selector(V_A), Some(V_A));
        assert_eq!(policy.producible_selector(V_B), Some(V_B));
    }

    #[test]
    fn class_pick_follows_declared_preference_not_incidental_order() {
        // Reversing the declared order flips which version the class id resolves to, proving the
        // pick is the first-declared producible entry — deterministic, not map-iteration order.
        let policy = RouterPolicy::new(registry(), vec![(V_B, V_B), (V_A, V_A)], vec![ASSESSOR]);
        assert_eq!(policy.producible_selector(V_CLASS), Some(V_B));
    }

    #[test]
    fn supported_selectors_include_sentinel_when_default_class_supported() {
        // V_CLASS is the default class and is fully supported (producible entry + assessor), so the
        // chain-default sentinel is serveable and advertised, alongside the entry pins and class id.
        let policy = RouterPolicy::new(registry(), vec![(V_A, V_A), (V_B, V_B)], vec![ASSESSOR]);
        let supported = policy.supported_selectors();
        assert!(supported.contains(&UNSPECIFIED_SELECTOR), "sentinel must be supported");
        assert!(supported.contains(&V_A));
        assert!(supported.contains(&V_B));
        assert!(supported.contains(&V_CLASS));
    }

    #[test]
    fn supported_selectors_omit_sentinel_when_default_class_not_producible() {
        // Default class V_CLASS has a reachable assessor, but the backend produces nothing in it,
        // so it is not in supported_classes and the sentinel must NOT be advertised — otherwise the
        // broker would accept a chain-default request it cannot fulfill.
        let policy = RouterPolicy::new(registry(), vec![], vec![ASSESSOR]);
        let supported = policy.supported_selectors();
        assert!(!supported.contains(&UNSPECIFIED_SELECTOR), "sentinel must not be supported");
        assert!(supported.is_empty());
    }
}
