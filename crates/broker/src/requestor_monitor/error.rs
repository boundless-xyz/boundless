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

//! Error type for the requestor monitor service.

use thiserror::Error;

use crate::coded_error_impl;
use crate::errors::CodedError;

#[derive(Error)]
pub enum MonitorError {
    #[error("Failed to lock internal state")]
    LockFailed,
}

coded_error_impl!(MonitorError, "RM",
    LockFailed => "4001",
);
