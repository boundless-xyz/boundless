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

//! Shared chain-monitor query interface — implemented by both the default
//! [`ChainMonitorV2`](super::ChainMonitorV2) and the legacy
//! [`ChainMonitorService`](super::ChainMonitorService).

use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;

#[derive(Clone, Debug, Copy)]
pub(crate) struct ChainHead {
    pub block_number: u64,
    pub block_timestamp: u64,
}

/// Trait abstracting the chain monitor query interface.
/// Implemented by both the default `ChainMonitorV2` (eth_getBlockReceipts) and the
/// legacy `ChainMonitorService` pair (eth_getLogs, selected via `[market] rpc_mode = "legacy"`).
#[async_trait]
pub(crate) trait ChainMonitorApi: Send + Sync {
    async fn current_chain_head(&self) -> Result<ChainHead>;
    async fn current_gas_price(&self) -> Result<u128>;
}

/// Type alias for a heap-allocated, type-erased chain monitor.
pub(crate) type ChainMonitorObj = Arc<dyn ChainMonitorApi>;
