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

//! Commands for CLI setup and configuration.

mod custom_networks;
mod network;
pub mod secrets;
#[allow(clippy::module_inception)]
mod setup;

pub use custom_networks::{
    clone_prebuilt_market_as_custom, clone_prebuilt_rewards_as_custom,
    create_minimal_custom_network, create_minimal_custom_rewards, list_networks, rename_network,
    setup_custom_market, setup_custom_rewards, update_custom_market_addresses,
    update_custom_rewards_addresses,
};
pub use network::{
    get_prebuilt_networks, is_prebuilt_network, normalize_market_network,
    normalize_rewards_network, query_chain_id, ModuleType, PREBUILT_PROVER_NETWORKS,
    PREBUILT_REQUESTOR_NETWORKS, PREBUILT_REWARDS_NETWORKS,
};
pub use secrets::{address_from_private_key, merge_optional, process_private_key};
pub use setup::{ProverSetup, RequestorSetup, RewardsSetup, SetupInteractive};
