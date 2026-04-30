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

//! Error type for the market monitor service.

use thiserror::Error;

use crate::coded_error_impl;
use crate::errors::CodedError;

#[derive(Error)]
pub enum MarketMonitorErr {
    #[error("{code} Event polling failed: {0:#}", code = self.code())]
    EventPollingErr(anyhow::Error),

    #[allow(dead_code)]
    #[error("{code} Log processing failed: {0:#}", code = self.code())]
    LogProcessingFailed(anyhow::Error),

    #[error("{code} Unexpected error: {0:#}", code = self.code())]
    UnexpectedErr(#[from] anyhow::Error),

    #[error("{code} Receiver dropped", code = self.code())]
    ReceiverDropped,
}

coded_error_impl!(MarketMonitorErr, "MM",
    EventPollingErr(..)     => "501",
    LogProcessingFailed(..) => "502",
    UnexpectedErr(..)       => "500",
    ReceiverDropped         => "502",
);
