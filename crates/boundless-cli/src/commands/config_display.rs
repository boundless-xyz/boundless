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

//! Shared utilities for displaying configuration status across modules

use crate::commands::setup::secrets::address_from_private_key;
use crate::display::{obscure_url, DisplayManager};
use alloy::primitives::Address;

/// Module type for configuration display
#[derive(Debug, Clone, Copy)]
pub enum ModuleType {
    /// Requestor module
    Requestor,
    /// Prover module
    Prover,
    /// Rewards module
    Rewards,
}

impl ModuleType {
    /// Get the display name for this module
    pub fn display_name(&self) -> &'static str {
        match self {
            ModuleType::Requestor => "Requestor Module",
            ModuleType::Prover => "Prover Module",
            ModuleType::Rewards => "Rewards Module",
        }
    }

    /// Get the setup command for this module
    pub fn setup_command(&self) -> &'static str {
        match self {
            ModuleType::Requestor => "boundless requestor setup",
            ModuleType::Prover => "boundless prover setup",
            ModuleType::Rewards => "boundless rewards setup",
        }
    }

    /// Get the help command for this module
    pub fn help_command(&self) -> &'static str {
        match self {
            ModuleType::Requestor => "boundless requestor --help",
            ModuleType::Prover => "boundless prover --help",
            ModuleType::Rewards => "boundless rewards --help",
        }
    }

    /// Get the environment variable name for private key
    pub fn private_key_env_var(&self) -> &'static str {
        match self {
            ModuleType::Requestor => "REQUESTOR_PRIVATE_KEY",
            ModuleType::Prover => "PROVER_PRIVATE_KEY",
            ModuleType::Rewards => "REWARD_PRIVATE_KEY",
        }
    }

    /// Get the environment variable name for RPC URL
    pub fn rpc_url_env_var(&self) -> &'static str {
        match self {
            ModuleType::Requestor => "REQUESTOR_RPC_URL",
            ModuleType::Prover => "PROVER_RPC_URL",
            ModuleType::Rewards => "REWARD_RPC_URL",
        }
    }

    /// Get the label for address display
    pub fn address_label(&self) -> &'static str {
        match self {
            ModuleType::Requestor => "Requestor Address",
            ModuleType::Prover => "Prover Address",
            ModuleType::Rewards => "Reward Address",
        }
    }
}

/// Normalize network name for display
pub fn normalize_network_name(network: &str) -> &str {
    match network {
        "base-mainnet" => "Base Mainnet",
        "base-sepolia" => "Base Sepolia",
        "eth-mainnet" => "Ethereum Mainnet",
        "eth-sepolia" => "Ethereum Sepolia",
        "mainnet" => "Ethereum Mainnet",
        "sepolia" => "Ethereum Sepolia",
        custom => custom,
    }
}

/// Get private key from environment or config, with source tracking
pub fn get_private_key_with_source<'a>(
    env_var_name: &str,
    config_pk: Option<&'a str>,
) -> (Option<&'a str>, &'static str) {
    if let Ok(env_pk) = std::env::var(env_var_name) {
        (Some(Box::leak(env_pk.into_boxed_str())), "env")
    } else {
        (config_pk, "config")
    }
}

/// Get RPC URL from environment or config, with source tracking
pub fn get_rpc_url_with_source<'a>(
    env_var_name: &str,
    config_rpc: Option<&'a str>,
) -> (Option<&'a str>, &'static str) {
    if let Ok(env_url) = std::env::var(env_var_name) {
        (Some(Box::leak(env_url.into_boxed_str())), "env")
    } else {
        (config_rpc, "config")
    }
}

/// Display RPC URL with obscuring and source
pub fn display_rpc_url(display: &DisplayManager, rpc_url: Option<&str>, source: &str) {
    if let Some(url) = rpc_url {
        display.item_colored("RPC URL", format!("{} [{}]", obscure_url(url), source), "dimmed");
    }
}

/// Display address and private key status
pub fn display_address_and_key_status(
    display: &DisplayManager,
    label: &str,
    pk: Option<&str>,
    pk_source: &str,
    config_addr: Option<&str>,
) {
    if let Some(pk_str) = pk {
        if let Some(addr) = address_from_private_key(pk_str) {
            display.item_colored(label, format!("{:#x}", addr), "green");
        }
        display.item("Private Key", format!("Configured [{}]", pk_source));
    } else if let Some(addr) = config_addr {
        display.item_colored(label, addr, "green");
        display.item_colored("Private Key", "Not configured (read-only) [config file]", "yellow");
    } else {
        display.item_colored("Private Key", "Not configured (read-only)", "yellow");
    }
}

/// Display "not configured" error for a module
pub fn display_not_configured(display: &DisplayManager, module: ModuleType) {
    display.warning(&format!("Not configured - run '{}'", module.setup_command()));
}

/// Display tip message for a module
pub fn display_tip(display: &DisplayManager, module: ModuleType) {
    display.note(&format!("💡 Tip: Run '{}' to see available commands", module.help_command()));
}

/// Get address from private key as Address type
pub fn address_from_pk(pk: &str) -> Option<Address> {
    address_from_private_key(pk)
}
