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

use alloy::primitives::FixedBytes;
use boundless_market::contracts::{RouterEntry, RouterRegistry};

/// A [`RouterRegistry`] snapshot combined with the broker's capability inputs — which verifier
/// selectors it can produce and which assessor selectors it can satisfy, in priority order —
/// resolved into the policy questions the broker asks per order and per batch: which verifier
/// selector to emit for a signed selector, which assessor seals a batch, and which selectors /
/// classes are supported at all. All methods are pure in-memory lookups.
///
/// Built once at startup and shared by everything that needs it (backend, batch processor);
/// tests build it from an in-memory registry fixture so they exercise the same resolution logic
/// as production.
#[derive(Clone, Debug)]
pub struct RouterPolicy {
    registry: RouterRegistry,
    /// Candidate assessor selectors in descending priority; the first registered in a verifier
    /// class's `requiredAssessorClass` wins.
    assessor_priority: Vec<FixedBytes<4>>,
    /// Verifier class id -> the verifier selector the broker produces for orders in that class.
    /// A requestor may sign a class id and the broker still emits a producible entry in it.
    class_to_producible: HashMap<FixedBytes<4>, FixedBytes<4>>,
    /// Verifier classes the broker fully supports: it can produce a verifier entry in the class
    /// and a candidate assessor is registered in the class's `requiredAssessorClass`.
    supported_classes: Vec<FixedBytes<4>>,
}

impl RouterPolicy {
    /// Derives the policy from a registry snapshot and the broker's capabilities. `producible`
    /// pairs each verifier selector the broker can serve with the selector it emits in a seal;
    /// when one class registers several producible entries, the last pair wins
    /// [`Self::producible_selector`]. `assessor_priority` lists the candidate assessor selectors
    /// in descending preference.
    pub fn new(
        registry: RouterRegistry,
        producible: Vec<(FixedBytes<4>, FixedBytes<4>)>,
        assessor_priority: Vec<FixedBytes<4>>,
    ) -> Self {
        let mut class_to_producible = HashMap::new();
        for (selector, produced) in producible {
            if let Some(entry) = registry.entry(selector) {
                class_to_producible.insert(entry.class_id, produced);
            }
        }
        let mut policy = Self {
            registry,
            assessor_priority,
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

    /// The verifier selector the broker produces for orders in `signed`'s class, when `signed` is
    /// a verifier class id; `None` otherwise (entry selectors and the default sentinel pass
    /// through unmapped).
    pub fn producible_selector(&self, signed: FixedBytes<4>) -> Option<FixedBytes<4>> {
        self.class_to_producible.get(&signed).copied()
    }

    /// The verifier classes the broker fully supports (producible verifier entry present AND a
    /// candidate assessor registered in the class's `requiredAssessorClass`).
    pub fn supported_classes(&self) -> &[FixedBytes<4>] {
        &self.supported_classes
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
