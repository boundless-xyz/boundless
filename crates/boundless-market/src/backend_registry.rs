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

//! Selector → backend registries used by broker pricing & proving paths.
//!
//! - `ProverRegistry`: selector → `Prover`. Used for cycle-counting /
//!   preflight only (SDK pricing + broker `OrderPricer`).
//! - `BackendRegistry`: selector → `BackendEntry` (`Prover` +
//!   `BackendProvider`). Used by broker proving / aggregator / submitter
//!   paths that need composition + on-chain sealing.

use std::{collections::HashMap, sync::Arc};

use super::backend_provider::BackendProviderObj;
use crate::prover_utils::prover::ProverObj;

/// One backend registered with the broker for full proving.
///
/// One entry can claim multiple selectors and is shared under each
/// selector key. `#[non_exhaustive]`; use [`Self::new`] to construct.
#[derive(Clone)]
#[non_exhaustive]
pub struct BackendEntry {
    /// Display name (logs / metrics labels).
    pub name: String,
    /// Selectors this backend handles (`request.requirements.selector`).
    pub selectors: Vec<crate::VerifierSelector>,
    /// Broker `Prover` for preflight + primitive proving.
    pub prover: ProverObj,
    /// `BackendProvider` for per-selector composition + on-chain sealing.
    pub provider: BackendProviderObj,
}

impl BackendEntry {
    /// Construct a `BackendEntry`. Required because the struct is
    /// `#[non_exhaustive]` (out-of-crate struct-literal construction is
    /// blocked).
    pub fn new(
        name: impl Into<String>,
        selectors: Vec<crate::VerifierSelector>,
        prover: ProverObj,
        provider: BackendProviderObj,
    ) -> Self {
        Self { name: name.into(), selectors, prover, provider }
    }
}

/// Selector → `BackendEntry` registry. Used by paths that need
/// composition + on-chain sealing.
#[derive(Clone, Default)]
#[non_exhaustive]
pub struct BackendRegistry {
    by_selector: HashMap<crate::VerifierSelector, Arc<BackendEntry>>,
    backends: Vec<Arc<BackendEntry>>,
}

impl BackendRegistry {
    /// Construct an empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Construct a registry with a single backend registered for all listed selectors.
    pub fn with_backend(registration: BackendEntry) -> Self {
        let mut registry = Self::default();
        registry.register(registration);
        registry
    }

    /// Register a backend.
    ///
    /// Conflict resolution when two registrations claim the same selector
    /// is unspecified and may change; callers should not rely on it. Treat
    /// duplicate-selector registrations as configuration bugs.
    pub fn register(&mut self, registration: BackendEntry) {
        let registration = Arc::new(registration);
        for selector in &registration.selectors {
            if let Some(prev) = self.by_selector.insert(*selector, registration.clone()) {
                tracing::warn!(
                    selector = %hex::encode(selector.0),
                    new_backend = %registration.name,
                    previous_backend = %prev.name,
                    "BackendRegistry: selector already registered; overwriting binding"
                );
            }
        }
        self.backends.push(registration);
    }

    /// Look up the backend registered for the given selector.
    pub fn find(&self, selector: crate::VerifierSelector) -> Option<&BackendEntry> {
        self.by_selector.get(&selector).map(Arc::as_ref)
    }

    /// All registered backends.
    pub fn backends(&self) -> &[Arc<BackendEntry>] {
        &self.backends
    }

    /// Number of registered backends.
    pub fn len(&self) -> usize {
        self.backends.len()
    }

    /// Whether the registry is empty.
    pub fn is_empty(&self) -> bool {
        self.backends.is_empty()
    }

    /// Project into a [`ProverRegistry`] view (selectors mapped to provers
    /// only, drops the `BackendProvider` half).
    pub fn prover_view(&self) -> ProverRegistry {
        let mut provers = ProverRegistry::default();
        for entry in &self.backends {
            provers.register(entry.name.clone(), entry.selectors.clone(), entry.prover.clone());
        }
        provers
    }

    /// Build a [`crate::selector::SupportedSelectors`] from every selector
    /// claimed by any registered backend. Each selector is classified via
    /// [`crate::selector::classify_selector`].
    pub fn supported_selectors(&self) -> crate::selector::SupportedSelectors {
        let mut supported = crate::selector::SupportedSelectors::new();
        for selector in self.by_selector.keys() {
            supported.add_selector(*selector, crate::selector::classify_selector(*selector));
        }
        supported
    }
}

/// Selector → `Prover` registry. Used by paths that only need
/// cycle-counting / preflight (SDK pricing, broker `OrderPricer`).
#[derive(Clone, Default)]
#[non_exhaustive]
pub struct ProverRegistry {
    by_selector: HashMap<crate::VerifierSelector, ProverObj>,
    /// Display metadata for registered provers, kept for logging purposes.
    /// Never read on the lookup path.
    metadata: Vec<ProverMetadata>,
}

#[derive(Clone)]
struct ProverMetadata {
    #[allow(dead_code)]
    name: String,
    #[allow(dead_code)]
    selectors: Vec<crate::VerifierSelector>,
}

impl ProverRegistry {
    /// Construct an empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Construct a registry with a single prover registered for all listed selectors.
    pub fn with_prover(
        name: impl Into<String>,
        selectors: Vec<crate::VerifierSelector>,
        prover: ProverObj,
    ) -> Self {
        let mut registry = Self::default();
        registry.register(name, selectors, prover);
        registry
    }

    /// Register a prover for a set of selectors.
    ///
    /// If a selector is already claimed, the new registration replaces the
    /// old binding for that selector and a `tracing::warn` is emitted.
    pub fn register(
        &mut self,
        name: impl Into<String>,
        selectors: Vec<crate::VerifierSelector>,
        prover: ProverObj,
    ) {
        let name = name.into();
        for selector in &selectors {
            if let Some(_prev) = self.by_selector.insert(*selector, prover.clone()) {
                tracing::warn!(
                    selector = %hex::encode(selector.0),
                    new_backend = %name,
                    "ProverRegistry: selector already registered; overwriting binding"
                );
            }
        }
        self.metadata.push(ProverMetadata { name, selectors });
    }

    /// Look up the prover registered for the given selector.
    pub fn find(&self, selector: crate::VerifierSelector) -> Option<&ProverObj> {
        self.by_selector.get(&selector)
    }

    /// Iterator over every registered selector.
    pub fn selectors(&self) -> impl Iterator<Item = crate::VerifierSelector> + '_ {
        self.by_selector.keys().copied()
    }

    /// Number of registered provers (counting one per registration call,
    /// not per selector).
    pub fn len(&self) -> usize {
        self.metadata.len()
    }

    /// Whether the registry is empty.
    pub fn is_empty(&self) -> bool {
        self.metadata.is_empty()
    }
}
