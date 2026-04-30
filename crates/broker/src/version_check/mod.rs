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

//! Minimum broker version enforcement via the on-chain VersionRegistry
//! contract.
//!
//! The broker checks the on-chain minimum version at startup and
//! periodically via a supervised background task ([`VersionCheckTask`]).
//! If the broker version is below the minimum, the supervisor faults,
//! triggering graceful shutdown. If the contract cannot be read (RPC
//! error, not deployed, etc.), it logs a warning and continues.
//!
//! Layout:
//! - [`service`] — [`VersionCheckTask`], the
//!   [`BrokerService`](crate::task::BrokerService) `run` loop, the
//!   `VERSION_REGISTRIES` lookup table, the `IVersionRegistry` ABI binding,
//!   and the version-packing helpers.
//! - `error` — [`VersionCheckError`].

mod error;
mod service;

pub(crate) use service::VersionCheckTask;
pub use service::{format_version, pack_version, unpack_version, BROKER_VERSION};
