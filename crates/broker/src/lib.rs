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

use boundless_market::contracts::ProofRequest;
pub use config::Config;
pub use config::ConfigLock;
pub use rpc_retry_policy::CustomRetryPolicy;
pub(crate) use storage::ConfigurableDownloader;

pub(crate) mod aggregator;
pub use aggregator::{AggregationState, Batch, BatchStatus};
pub mod args;
mod broker;
pub(crate) mod chain_monitor_v2;
pub(crate) mod channels;
mod db;
pub use db::{broker_sqlite_url_for_chain, DbObj, SqliteDb};
pub(crate) mod market_monitor;
pub(crate) mod offchain_market_monitor;
pub(crate) mod order_committer;
pub(crate) mod order_evaluator;
pub(crate) mod order_locker;
pub(crate) mod order_pricer;
mod order_types;
pub mod provers;
pub(crate) mod proving;
pub(crate) mod requestor_monitor;
pub(crate) mod service_runner;
pub(crate) mod shared;
pub use shared::config;
pub(crate) use shared::{errors, prioritization, task};
pub(crate) mod submitter;
pub(crate) mod telemetry;
pub mod utils;
pub(crate) use utils::{
    format_expiries, is_dev_mode, now_timestamp, reaper, rpc_retry_policy, storage,
};
pub use utils::{futures_retry, rpcmetrics, sequential_fallback};
pub mod version_check;

pub use args::{build_chain_provider, ChainArgs, ChainPipeline, CoreArgs};
pub use broker::{resolve_deployment, Broker};

pub use boundless_market::prover_utils::{
    Erc1271GasCache, FulfillmentType, MarketConfig, OrderPricingContext, OrderPricingError,
    OrderPricingOutcome, OrderRequest, PreflightCache, PreflightCacheKey, PreflightCacheValue,
    ProveLimitReason,
};
pub(crate) use order_types::proving_order_from_request;
pub use order_types::{CompressionType, Order, OrderStateChange, OrderStatus};

#[cfg(feature = "test-utils")]
pub mod test_utils;

#[cfg(test)]
pub mod tests;
