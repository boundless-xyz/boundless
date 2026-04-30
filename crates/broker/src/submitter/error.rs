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

//! Error type for the submitter service.

use boundless_market::contracts::boundless_market::MarketError;
use thiserror::Error;

use crate::coded_error_impl;
use crate::errors::CodedError;

#[derive(Error)]
pub enum SubmitterErr {
    #[error("{code} Batch submission failed: {0:?}", code = self.code())]
    BatchSubmissionFailed(Vec<Self>),

    #[error("{code} Batch submission failed due to timeouts: {0:?}", code = self.code())]
    BatchSubmissionFailedTimeouts(Vec<Self>),

    #[error("{code} Failed to confirm transaction: {0}", code = self.code())]
    TxnConfirmationError(MarketError),

    #[error("{code} All requests expired before submission: {0:?}", code = self.code())]
    AllRequestsExpiredBeforeSubmission(Vec<String>),

    #[error("{code} Some requests expired before submission: {0:?}", code = self.code())]
    SomeRequestsExpiredBeforeSubmission(Vec<String>),

    #[error("{code} Market error: {0}", code = self.code())]
    MarketError(#[from] MarketError),

    #[error("{code} Unexpected error: {0:#}", code = self.code())]
    UnexpectedErr(#[from] anyhow::Error),
}

coded_error_impl!(SubmitterErr, "SUB",
    UnexpectedErr(..)                       => "500",
    AllRequestsExpiredBeforeSubmission(..)  => "001",
    SomeRequestsExpiredBeforeSubmission(..) => "005",
    MarketError(..)                         => "002",
    BatchSubmissionFailed(..)               => "004",
    BatchSubmissionFailedTimeouts(..)       => "003",
    TxnConfirmationError(..)                => "006",
);
