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

//! Test utilities for the Boundless project.
//!
//! This crate provides common testing functionality used across multiple
//! Boundless crates, including contract deployment utilities, test contexts,
//! and mock receipt generation.

pub mod market;
#[cfg(feature = "povw")]
pub mod povw;
pub mod verifier;
#[cfg(feature = "zkc")]
pub mod zkc;

pub mod guests {
    // Export image IDs and paths publicly to ensure all dependants use the same ones.
    pub use guest_assessor::{ASSESSOR_GUEST_ELF, ASSESSOR_GUEST_ID, ASSESSOR_GUEST_PATH};
    pub use guest_set_builder::{SET_BUILDER_ELF, SET_BUILDER_ID, SET_BUILDER_PATH};
    pub use guest_util::{
        ECHO_ELF, ECHO_ID, ECHO_PATH, IDENTITY_ELF, IDENTITY_ID, IDENTITY_PATH, LOOP_ELF, LOOP_ID,
        LOOP_PATH,
    };
}
