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

//! Library core of `boundless-executor`.
//!
//! Exposes a small [`Executor`] trait plus one implementation per supported
//! zkVM, and a Bonsai-compatible HTTP API (`api`) that drives those backends.
//! Each backend is gated behind a cargo feature so consumers only pay for
//! the SDKs they actually need.

pub mod api;
pub mod backend;
pub mod backends;
pub mod session;
pub mod storage;

pub use backend::{ExecutionStats, Executor, ExecutorError, Registry, ZkvmKind};
pub use storage::Storage;
