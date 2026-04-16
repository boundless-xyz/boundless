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

//! zkVM backend implementations. Each backend is feature-gated so SDKs are
//! only pulled in when the corresponding feature is enabled.
//!
//! ## Adding a new backend
//!
//! 1. Add a new variant to [`crate::backend::ZkvmKind`] and update its
//!    `Display` impl.
//! 2. Add a feature flag + optional SDK dependency to `Cargo.toml`.
//! 3. Create `src/backends/<name>.rs` implementing the [`Executor`] trait.
//! 4. Feature-gate the `pub mod <name>;` below and register the backend in
//!    [`default_registry`] under the matching `#[cfg(feature = "<name>")]`.
//!
//! The API layer is backend-agnostic — nothing else needs to change.

#[cfg(feature = "risc0")]
pub mod risc0;

use std::sync::Arc;

use crate::backend::{Executor, Registry};

/// Build a [`Registry`] containing every backend enabled at compile time.
///
/// Backend constructors that need runtime resources (e.g. locating `r0vm`)
/// may fail; on failure the backend is omitted and a warning is logged so
/// the rest of the service can still start.
pub fn default_registry() -> Registry {
    #[allow(unused_mut)]
    let mut reg = Registry::new();

    #[cfg(feature = "risc0")]
    {
        match risc0::Risc0Executor::new() {
            Ok(exec) => reg = reg.with(Arc::new(exec) as Arc<dyn Executor>),
            Err(e) => tracing::warn!(error = %e, "risc0 backend disabled"),
        }
    }

    reg
}
