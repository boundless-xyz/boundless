// Copyright 2025 RISC Zero, Inc.
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

#[cfg(not(target_os = "zkvm"))]
use crate::{
    balance_alerts_layer::BalanceAlertProvider, dynamic_gas_filler::DynamicGasFiller,
    nonce_layer::NonceProvider,
};
#[cfg(not(target_os = "zkvm"))]
use alloy::providers::fillers::{ChainIdFiller, JoinFill};
#[cfg(not(target_os = "zkvm"))]
use alloy::providers::{Identity, RootProvider};

/// Type used in the [Client] and [StandardRequestBuilder] to indicate that the component in question is not provided.
///
/// Note that this in an [uninhabited type] and cannot be instantiated. When used as
/// `Option<NotProvided>`, the only possible variant for this option is `None`.
///
/// [uninhabited type]: https://smallcultfollowing.com/babysteps/blog/2018/08/13/never-patterns-exhaustive-matching-and-uninhabited-types-oh-my/
/// [StandardRequestBuilder]: crate::request_builder::StandardRequestBuilder
/// [Client]: crate::client::Client
#[derive(Copy, Clone, Debug)]
pub enum NotProvided {}

/// Alias for the [alloy] RPC provider used by the [StandardClient][crate::client::StandardClient]
/// and [StandardRequestBuilder][crate::request_builder::StandardRequestBuilder]
#[cfg(not(target_os = "zkvm"))]
pub type StandardRpcProvider = NonceProvider<
    JoinFill<JoinFill<Identity, ChainIdFiller>, DynamicGasFiller>,
    BalanceAlertProvider<RootProvider>,
>;

/// A very small utility function to get the current unix timestamp.
// TODO(#379): Avoid drift relative to the chain's timestamps.
#[cfg(not(target_os = "zkvm"))]
pub(crate) fn now_timestamp() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_secs()
}
