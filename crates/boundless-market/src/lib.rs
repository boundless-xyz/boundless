// Copyright 2024 RISC Zero, Inc.
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

/// Re-export of [alloy], provided to ensure that the correct version of the types used in the
/// public API are available in case multiple versions of [alloy] are in use.
///
/// Because [alloy] is a v0.x crate, it is not covered under the semver policy of this crate.
#[cfg(not(target_os = "zkvm"))]
pub use alloy;

#[cfg(not(target_os = "zkvm"))]
pub mod client;
pub mod contracts;
#[cfg(not(target_os = "zkvm"))]
pub mod input;
#[cfg(not(target_os = "zkvm"))]
pub mod order_stream_client;
#[cfg(not(target_os = "zkvm"))]
pub mod storage;
