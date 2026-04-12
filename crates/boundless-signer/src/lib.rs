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

//! Pluggable transaction-signing backend for the Boundless broker and CLI.
//!
//! # Overview
//!
//! The broker currently embeds private keys directly.  On cloud GPU
//! marketplace instances the key is exposed to an untrusted environment.
//! This crate introduces a [`SignerBackend`] trait so the broker and CLI can
//! route signing calls to an alternative implementation – in particular the
//! fleet-node HTTP endpoint that owns the WalletConnect session.
//!
//! # Backends
//!
//! | Backend | Feature | Description |
//! |---|---|---|
//! | [`LocalSignerBackend`] | (default) | Wraps `alloy::signers::local::PrivateKeySigner`. Zero-regression path. |
//! | [`HttpRemoteSignerBackend`] | `http-remote` | Calls fleet-node `POST /sign`. |
//!
//! # Configuration
//!
//! Use [`from_config`] to construct a backend from a [`SignerConfig`] TOML
//! section.  When no explicit config is provided, the factory falls back to the
//! `PROVER_PRIVATE_KEY` / `REWARD_PRIVATE_KEY` env vars (existing behaviour).

pub mod backend;
pub mod bridge;
pub mod config;
#[cfg(feature = "http-remote")]
pub mod http_remote;
pub mod local;
pub mod traits;
pub mod types;

pub use backend::ConcreteSignerBackend;
pub use bridge::SignerBackendBridge;
pub use config::{from_config, HttpSignerConfig, RoleSignerConfig, SignerConfig};
#[cfg(feature = "http-remote")]
pub use http_remote::HttpRemoteSignerBackend;
pub use local::LocalSignerBackend;
pub use traits::{SignerBackend, SignerMode};
pub use types::{SignerError, SignerRole, TransactionIntent};
