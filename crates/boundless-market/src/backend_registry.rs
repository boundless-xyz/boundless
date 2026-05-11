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

//! Selector → backend registry used by broker pricing & proving paths.
//!
//! `BackendRegistry` is the single selector router for the broker. Pricing
//! uses the entry's `prover`; proving, aggregation, and submission also use
//! the entry's `BackendProvider`.

use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};

use super::backend_provider::BackendProviderObj;
use crate::prover_utils::prover::ProverObj;

/// Errors returned when constructing selector registries.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum RegistryError {
    /// More than one registration tried to claim the same selector.
    #[error(
        "selector {selector} already registered to {existing_registration}; cannot also register {new_registration}"
    )]
    DuplicateSelector {
        /// Selector encoded as hex.
        selector: String,
        /// Registration that already owns the selector.
        existing_registration: String,
        /// Registration that attempted to claim the selector.
        new_registration: String,
    },
}

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
        Self::try_with_backend(registration).expect("duplicate selector in BackendRegistry")
    }

    /// Construct a registry with a single backend registered for all listed selectors.
    pub fn try_with_backend(registration: BackendEntry) -> Result<Self, RegistryError> {
        let mut registry = Self::default();
        registry.try_register(registration)?;
        Ok(registry)
    }

    /// Register a backend.
    ///
    /// Panics if any selector is already registered. Use
    /// [`Self::try_register`] to handle duplicate selectors explicitly.
    pub fn register(&mut self, registration: BackendEntry) {
        self.try_register(registration).expect("duplicate selector in BackendRegistry")
    }

    /// Register a backend.
    ///
    /// Duplicate-selector registrations are configuration bugs and are
    /// rejected before any registry state is mutated.
    pub fn try_register(&mut self, registration: BackendEntry) -> Result<(), RegistryError> {
        let mut seen = HashSet::new();
        for selector in &registration.selectors {
            if !seen.insert(*selector) {
                return Err(RegistryError::DuplicateSelector {
                    selector: hex::encode(selector.0),
                    existing_registration: registration.name.clone(),
                    new_registration: registration.name.clone(),
                });
            }
            if let Some(prev) = self.by_selector.get(selector) {
                return Err(RegistryError::DuplicateSelector {
                    selector: hex::encode(selector.0),
                    existing_registration: prev.name.clone(),
                    new_registration: registration.name.clone(),
                });
            }
        }

        let registration = Arc::new(registration);
        for selector in &registration.selectors {
            self.by_selector.insert(*selector, registration.clone());
        }
        self.backends.push(registration);
        Ok(())
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
