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

//! Error type for the order locker service.

use thiserror::Error;

use crate::coded_error_impl;
use crate::errors::CodedError;

#[derive(Error)]
pub enum OrderLockerErr {
    #[error("{code} Failed to lock order: {0}", code = self.code())]
    LockTxFailed(String),

    #[error("{code} Failed to confirm lock tx: {0}", code = self.code())]
    LockTxNotConfirmed(String),

    #[error("{code} Insufficient balance for lock", code = self.code())]
    InsufficientBalance,

    #[error("{code} Order already locked", code = self.code())]
    AlreadyLocked,

    #[error("{code} RPC error: {0:?}", code = self.code())]
    RpcErr(anyhow::Error),

    #[error("{code} Unexpected error: {0:?}", code = self.code())]
    UnexpectedError(#[from] anyhow::Error),

    /// Pre-lock gas check failed (e.g. gas spike). Caller may retry next block.
    #[error("{code} Pre-lock gas check failed (retry later): {0}", code = self.code())]
    PreLockCheckRetry(String),
}

coded_error_impl!(OrderLockerErr, "OL",
    LockTxNotConfirmed(..)  => "006",
    LockTxFailed(..)        => "007",
    AlreadyLocked           => "009",
    InsufficientBalance     => "010",
    RpcErr(..)              => "011",
    PreLockCheckRetry(..)   => "012",
    UnexpectedError(..)     => "500",
);
