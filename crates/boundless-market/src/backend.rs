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

//! SDK-side backend marker trait.
//!
//! [`Backend`] is the type-level anchor for SDK-side companion traits keyed
//! on a zkVM, such as [`crate::order_pricer::BackendRequestPricerFactory`].

/// SDK-side marker trait for a zkVM backend.
///
/// Implementing types should be zero-sized markers. Stateful runtime
/// providers should implement [`crate::backend_provider::BackendProvider`]
/// instead. Companion traits keyed on `Backend` provide per-backend SDK
/// behavior.
pub trait Backend: Send + Sync + 'static {}
