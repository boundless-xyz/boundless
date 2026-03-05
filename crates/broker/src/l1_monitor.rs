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

//! Experimental L1Monitor implementation.
//!
//! Replaces both `ChainMonitorService` and `MarketMonitor` with a single struct when
//! `--experimental-rpc` is set. The implementation is a placeholder — all methods
//! will panic with `todo!("experimental-rpc")` until the logic is filled in.

use anyhow::Result;
use async_trait::async_trait;
use thiserror::Error;
use tokio_util::sync::CancellationToken;

use crate::{
    chain_monitor::{ChainHead, ChainMonitorApi},
    errors::CodedError,
    impl_coded_debug,
    task::{RetryRes, RetryTask, SupervisorErr},
};

#[derive(Error)]
pub enum L1MonitorErr {
    #[error("{code} Unexpected error: {0:?}", code = self.code())]
    UnexpectedErr(#[from] anyhow::Error),
}

impl_coded_debug!(L1MonitorErr);

impl CodedError for L1MonitorErr {
    fn code(&self) -> &str {
        match self {
            L1MonitorErr::UnexpectedErr(_) => "[B-L1M-500]",
        }
    }
}

/// Experimental replacement for `ChainMonitorService` + `MarketMonitor`.
///
/// Implements [`ChainMonitorApi`] (queried by `OrderPicker` and `OrderMonitor`) and
/// feeds the same `new_order_tx` / `order_state_tx` channels as `MarketMonitor`.
///
/// All methods are stubs — they panic until the experimental RPC logic is implemented.
pub(crate) struct L1Monitor;

impl L1Monitor {
    pub(crate) fn new() -> Self {
        todo!("experimental-rpc: L1Monitor::new not yet implemented")
    }

    pub(crate) async fn get_block_time(&self) -> Result<u64> {
        todo!("experimental-rpc: L1Monitor::get_block_time not yet implemented")
    }
}

#[async_trait]
impl ChainMonitorApi for L1Monitor {
    async fn current_chain_head(&self) -> Result<ChainHead> {
        todo!("experimental-rpc: L1Monitor::current_chain_head not yet implemented")
    }

    async fn current_gas_price(&self) -> Result<u128> {
        todo!("experimental-rpc: L1Monitor::current_gas_price not yet implemented")
    }
}

impl RetryTask for L1Monitor {
    type Error = L1MonitorErr;

    fn spawn(&self, _cancel_token: CancellationToken) -> RetryRes<Self::Error> {
        Box::pin(async {
            todo!("experimental-rpc: L1Monitor::spawn not yet implemented");
            #[allow(unreachable_code)]
            Ok::<(), SupervisorErr<L1MonitorErr>>(())
        })
    }
}
