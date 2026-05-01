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

//! Error type for the proving service.

use boundless_market::telemetry::CompletionOutcome;
use thiserror::Error;

use crate::coded_error_impl;
use crate::errors::CodedError;

#[derive(Error)]
pub enum ProvingErr {
    #[error("{code} Proving failed after retries: {0:#}", code = self.code())]
    ProvingFailed(anyhow::Error),

    #[error(
        "{code} Cancelled: secondary fulfillment order was fulfilled by another prover",
        code = self.code()
    )]
    CancelFulfilledByAnother,

    #[error("{code} Cancelled: order expired during proving", code = self.code())]
    CancelExpired,

    #[error(
        "{code} Proof completed but secondary fulfillment order was fulfilled by another prover",
        code = self.code()
    )]
    CompletedFulfilledByAnother,

    #[error("{code} Proof completed but order expired", code = self.code())]
    CompletedExpired,

    #[error("{code} Unexpected error: {0:#}", code = self.code())]
    UnexpectedError(#[from] anyhow::Error),
}

coded_error_impl!(ProvingErr, "PRO",
    ProvingFailed(..)            => "501",
    CancelFulfilledByAnother     => "502",
    CancelExpired                => "503",
    CompletedFulfilledByAnother  => "505",
    CompletedExpired             => "506",
    UnexpectedError(..)          => "500",
);

impl ProvingErr {
    pub(super) fn completion_outcome(&self) -> CompletionOutcome {
        match self {
            ProvingErr::ProvingFailed(_) | ProvingErr::UnexpectedError(_) => {
                CompletionOutcome::ProvingFailed
            }
            ProvingErr::CancelExpired | ProvingErr::CompletedExpired => {
                CompletionOutcome::ExpiredWhileProving
            }
            ProvingErr::CancelFulfilledByAnother | ProvingErr::CompletedFulfilledByAnother => {
                CompletionOutcome::Cancelled
            }
        }
    }
}
