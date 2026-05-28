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

//! Broker-facing backend boundary.
//!
//! The backend-agnostic API (traits, payloads, router) lives in the `boundless-backend` crate and
//! the RISC0 implementation in the `risc0-backend` crate; both are re-exported here.

pub use boundless_backend::*;
pub use risc0_backend::{prune_receipt_claim_journal, Risc0Backend, Risc0BackendConfig};
