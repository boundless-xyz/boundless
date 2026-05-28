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

//! Backend-agnostic API for the Boundless broker.
//!
//! Defines the [`Backend`]/[`BatchProcessor`] traits a proving backend implements, the neutral
//! command/payload types exchanged across that boundary, and the [`BackendRouter`] that
//! dispatches by verifier selector. Backend implementations (e.g. `risc0-backend`) depend on
//! this crate; the broker wires them together. No broker-internal types leak through here.

pub mod futures_retry;
mod router;
mod types;

pub use router::BackendRouter;
pub use types::*;
