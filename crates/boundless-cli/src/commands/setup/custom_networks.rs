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

//! Custom network management and configuration.

use anyhow::{bail, Context, Result};
use inquire::Text;

use super::network::{
    PREBUILT_PROVER_NETWORKS, PREBUILT_REQUESTOR_NETWORKS, PREBUILT_REWARDS_NETWORKS,
};
use super::SetupInteractive;
use crate::config_file::{Config, CustomMarketDeployment, CustomRewardsDeployment};
use crate::display::DisplayManager;

/// Create a new custom market deployment interactively
pub fn setup_custom_market() -> Result<CustomMarketDeployment> {
    let display = DisplayManager::new();
    print_section_header(&display, "Custom Market Deployment");

    let name = Text::new("Deployment name:")
        .with_help_message("A friendly name for this deployment (e.g., 'my-testnet')")
        .prompt()?;

    let chain_id: u64 = Text::new("Chain ID:")
        .with_help_message("EIP-155 chain ID (e.g., 84532 for Base Sepolia)")
        .prompt()?
        .parse()
        .context("Invalid chain ID")?;

    let boundless_market_address = Text::new("BoundlessMarket contract address:")
        .prompt()?
        .parse()
        .context("Invalid address")?;

    let verifier_router_address_str =
        Text::new("RiscZeroVerifierRouter address (optional, press Enter to skip):")
            .with_default("")
            .prompt()?;

    let verifier_router_address = if verifier_router_address_str.is_empty() {
        None
    } else {
        Some(verifier_router_address_str.parse().context("Invalid address")?)
    };

    let set_verifier_address =
        Text::new("RiscZeroSetVerifier address:").prompt()?.parse().context("Invalid address")?;

    let collateral_token_address_str =
        Text::new("Collateral token address (optional, press Enter to skip):")
            .with_default("")
            .prompt()?;

    let collateral_token_address = if collateral_token_address_str.is_empty() {
        None
    } else {
        Some(collateral_token_address_str.parse().context("Invalid address")?)
    };

    let order_stream_url_str =
        Text::new("Order stream URL (optional, press Enter to skip):").with_default("").prompt()?;

    let order_stream_url =
        if order_stream_url_str.is_empty() { None } else { Some(order_stream_url_str) };

    Ok(CustomMarketDeployment {
        name,
        chain_id,
        boundless_market_address,
        verifier_router_address,
        set_verifier_address,
        collateral_token_address,
        order_stream_url,
    })
}

/// Create a new custom rewards deployment interactively
pub fn setup_custom_rewards() -> Result<CustomRewardsDeployment> {
    let display = DisplayManager::new();
    print_section_header(&display, "Custom Rewards Deployment");

    let name = Text::new("Deployment name:")
        .with_help_message("A friendly name for this deployment (e.g., 'my-testnet')")
        .prompt()?;

    let chain_id: u64 = Text::new("Chain ID:")
        .with_help_message("EIP-155 chain ID")
        .prompt()?
        .parse()
        .context("Invalid chain ID")?;

    let zkc_address =
        Text::new("ZKC token contract address:").prompt()?.parse().context("Invalid address")?;

    let vezkc_address =
        Text::new("veZKC contract address:").prompt()?.parse().context("Invalid address")?;

    let staking_rewards_address = Text::new("Staking rewards contract address:")
        .prompt()?
        .parse()
        .context("Invalid address")?;

    let mining_accounting_address = Text::new("Mining accounting contract address:")
        .prompt()?
        .parse()
        .context("Invalid address")?;

    let mining_mint_address =
        Text::new("Mining mint contract address:").prompt()?.parse().context("Invalid address")?;

    Ok(CustomRewardsDeployment {
        name,
        chain_id,
        zkc_address,
        vezkc_address,
        staking_rewards_address,
        mining_accounting_address,
        mining_mint_address,
    })
}

/// Create a minimal custom market network with only chain ID set
pub fn create_minimal_custom_network(chain_id: u64) -> CustomMarketDeployment {
    use alloy::primitives::Address;

    CustomMarketDeployment {
        name: format!("custom-{}", chain_id),
        chain_id,
        boundless_market_address: Address::ZERO,
        verifier_router_address: None,
        set_verifier_address: Address::ZERO,
        collateral_token_address: None,
        order_stream_url: None,
    }
}

/// Create a minimal custom rewards deployment with only chain ID set
pub fn create_minimal_custom_rewards(chain_id: u64) -> CustomRewardsDeployment {
    use alloy::primitives::Address;

    CustomRewardsDeployment {
        name: format!("custom-{}", chain_id),
        chain_id,
        zkc_address: Address::ZERO,
        vezkc_address: Address::ZERO,
        staking_rewards_address: Address::ZERO,
        mining_accounting_address: Address::ZERO,
        mining_mint_address: Address::ZERO,
    }
}

/// Update custom market contract addresses from command-line flags
/// Returns true if any addresses were updated
pub fn update_custom_market_addresses(
    config: &mut Config,
    network_name: &str,
    setup: &SetupInteractive,
) -> Result<bool> {
    use alloy::primitives::Address;

    let custom_market = config
        .custom_markets
        .iter_mut()
        .find(|m| m.name == network_name)
        .with_context(|| format!("Custom market '{}' not found", network_name))?;

    let mut updated = false;

    if let Some(ref addr_str) = setup.boundless_market_address {
        custom_market.boundless_market_address =
            addr_str.parse::<Address>().context("Invalid boundless market address")?;
        updated = true;
    }

    if let Some(ref addr_str) = setup.verifier_router_address {
        custom_market.verifier_router_address =
            Some(addr_str.parse::<Address>().context("Invalid verifier router address")?);
        updated = true;
    }

    if let Some(ref addr_str) = setup.set_verifier_address {
        custom_market.set_verifier_address =
            addr_str.parse::<Address>().context("Invalid set verifier address")?;
        updated = true;
    }

    if let Some(ref addr_str) = setup.collateral_token_address {
        custom_market.collateral_token_address =
            Some(addr_str.parse::<Address>().context("Invalid collateral token address")?);
        updated = true;
    }

    if let Some(ref url) = setup.order_stream_url {
        custom_market.order_stream_url = Some(url.clone());
        updated = true;
    }

    Ok(updated)
}

/// Update custom rewards contract addresses from command-line flags
/// Returns true if any addresses were updated
pub fn update_custom_rewards_addresses(
    config: &mut Config,
    network_name: &str,
    setup: &SetupInteractive,
) -> Result<bool> {
    use alloy::primitives::Address;

    let custom_rewards = config
        .custom_rewards
        .iter_mut()
        .find(|r| r.name == network_name)
        .with_context(|| format!("Custom rewards deployment '{}' not found", network_name))?;

    let mut updated = false;

    if let Some(ref addr_str) = setup.zkc_address {
        custom_rewards.zkc_address = addr_str.parse::<Address>().context("Invalid ZKC address")?;
        updated = true;
    }

    if let Some(ref addr_str) = setup.vezkc_address {
        custom_rewards.vezkc_address =
            addr_str.parse::<Address>().context("Invalid veZKC address")?;
        updated = true;
    }

    if let Some(ref addr_str) = setup.staking_rewards_address {
        custom_rewards.staking_rewards_address =
            addr_str.parse::<Address>().context("Invalid staking rewards address")?;
        updated = true;
    }

    if let Some(ref addr_str) = setup.mining_accounting_address {
        custom_rewards.mining_accounting_address =
            addr_str.parse::<Address>().context("Invalid Mining accounting address")?;
        updated = true;
    }

    if let Some(ref addr_str) = setup.mining_mint_address {
        custom_rewards.mining_mint_address =
            addr_str.parse::<Address>().context("Invalid Mining mint address")?;
        updated = true;
    }

    Ok(updated)
}

/// Clone a pre-built market network as a custom network with optional overrides
pub fn clone_prebuilt_market_as_custom(
    config: &Config,
    prebuilt_name: &str,
    setup: &SetupInteractive,
) -> Result<CustomMarketDeployment> {
    use alloy::primitives::Address;

    let prebuilt = match prebuilt_name {
        "base-mainnet" => boundless_market::deployments::BASE,
        "base-sepolia" => boundless_market::deployments::BASE_SEPOLIA,
        "eth-sepolia" => boundless_market::deployments::SEPOLIA,
        _ => bail!("Unknown pre-built network: {}", prebuilt_name),
    };

    let custom_name = generate_unique_custom_name(config, prebuilt_name);

    let boundless_market_address = if let Some(ref addr_str) = setup.boundless_market_address {
        addr_str.parse::<Address>().context("Invalid boundless market address")?
    } else {
        prebuilt.boundless_market_address
    };

    let verifier_router_address = if let Some(ref addr_str) = setup.verifier_router_address {
        Some(addr_str.parse::<Address>().context("Invalid verifier router address")?)
    } else {
        prebuilt.verifier_router_address
    };

    let set_verifier_address = if let Some(ref addr_str) = setup.set_verifier_address {
        addr_str.parse::<Address>().context("Invalid set verifier address")?
    } else {
        prebuilt.set_verifier_address
    };

    let collateral_token_address = if let Some(ref addr_str) = setup.collateral_token_address {
        Some(addr_str.parse::<Address>().context("Invalid collateral token address")?)
    } else {
        prebuilt.collateral_token_address
    };

    let order_stream_url = if let Some(ref url) = setup.order_stream_url {
        Some(url.clone())
    } else {
        prebuilt.order_stream_url.as_ref().map(|cow| cow.to_string())
    };

    Ok(CustomMarketDeployment {
        name: custom_name,
        chain_id: prebuilt.market_chain_id.unwrap_or(0),
        boundless_market_address,
        verifier_router_address,
        set_verifier_address,
        collateral_token_address,
        order_stream_url,
    })
}

/// Clone a pre-built rewards network as a custom network with optional overrides
pub fn clone_prebuilt_rewards_as_custom(
    config: &Config,
    prebuilt_name: &str,
    setup: &SetupInteractive,
) -> Result<CustomRewardsDeployment> {
    use alloy::primitives::Address;

    let chain_id = match prebuilt_name {
        "eth-mainnet" => 1u64,
        "eth-sepolia" => 11155111u64,
        _ => bail!("Unknown pre-built rewards network: {}", prebuilt_name),
    };

    let prebuilt = boundless_zkc::deployments::Deployment::from_chain_id(chain_id)
        .ok_or_else(|| anyhow::anyhow!("No deployment found for chain ID: {}", chain_id))?;

    let custom_name = generate_unique_custom_name(config, prebuilt_name);

    let zkc_address = if let Some(ref addr_str) = setup.zkc_address {
        addr_str.parse::<Address>().context("Invalid ZKC address")?
    } else {
        prebuilt.zkc_address
    };

    let vezkc_address = if let Some(ref addr_str) = setup.vezkc_address {
        addr_str.parse::<Address>().context("Invalid veZKC address")?
    } else {
        prebuilt.vezkc_address
    };

    let staking_rewards_address = if let Some(ref addr_str) = setup.staking_rewards_address {
        addr_str.parse::<Address>().context("Invalid staking rewards address")?
    } else {
        prebuilt.staking_rewards_address
    };

    let mining_accounting_address = if let Some(ref addr_str) = setup.mining_accounting_address {
        addr_str.parse::<Address>().context("Invalid Mining accounting address")?
    } else {
        Address::ZERO
    };

    let mining_mint_address = if let Some(ref addr_str) = setup.mining_mint_address {
        addr_str.parse::<Address>().context("Invalid Mining mint address")?
    } else {
        Address::ZERO
    };

    Ok(CustomRewardsDeployment {
        name: custom_name,
        chain_id,
        zkc_address,
        vezkc_address,
        staking_rewards_address,
        mining_accounting_address,
        mining_mint_address,
    })
}

/// Generate a unique custom network name based on a base name
pub fn generate_unique_custom_name(config: &Config, base_name: &str) -> String {
    let mut name = format!("custom-{}", base_name);
    let mut counter = 1;

    while config.custom_markets.iter().any(|m| m.name == name)
        || config.custom_rewards.iter().any(|r| r.name == name)
    {
        name = format!("custom-{}-{}", base_name, counter);
        counter += 1;
    }

    name
}

/// List all available networks for a specific module
pub fn list_networks(config: &Config, module: Option<&str>) -> Result<()> {
    let display = DisplayManager::new();
    display.header("\nAvailable Networks");

    match module {
        Some("requestor") | None => {
            display.subsection("\nRequestor Networks:");
            display.note("Pre-built:");
            for network in PREBUILT_REQUESTOR_NETWORKS {
                let chain_id = match *network {
                    "base-mainnet" => 8453,
                    "base-sepolia" => 84532,
                    "eth-sepolia" => 11155111,
                    _ => 0,
                };
                display.info(&format!("  • {} (Chain ID: {})", network, chain_id));
            }

            if !config.custom_markets.is_empty() {
                display.note("Custom:");
                for market in &config.custom_markets {
                    display.info(&format!("  • {} (Chain ID: {})", market.name, market.chain_id));
                }
            }
        }
        _ => {}
    }

    match module {
        Some("prover") | None => {
            display.subsection("\nProver Networks:");
            display.note("Pre-built:");
            for network in PREBUILT_PROVER_NETWORKS {
                let chain_id = match *network {
                    "base-mainnet" => 8453,
                    "base-sepolia" => 84532,
                    "eth-sepolia" => 11155111,
                    _ => 0,
                };
                display.info(&format!("  • {} (Chain ID: {})", network, chain_id));
            }

            if !config.custom_markets.is_empty() {
                display.note("Custom:");
                for market in &config.custom_markets {
                    display.info(&format!("  • {} (Chain ID: {})", market.name, market.chain_id));
                }
            }
        }
        _ => {}
    }

    match module {
        Some("rewards") | None => {
            display.subsection("\nRewards Networks:");
            display.note("Pre-built:");
            for network in PREBUILT_REWARDS_NETWORKS {
                let chain_id = match *network {
                    "eth-mainnet" => 1,
                    "eth-sepolia" => 11155111,
                    _ => 0,
                };
                display.info(&format!("  • {} (Chain ID: {})", network, chain_id));
            }

            if !config.custom_rewards.is_empty() {
                display.note("Custom:");
                for rewards in &config.custom_rewards {
                    display.info(&format!("  • {} (Chain ID: {})", rewards.name, rewards.chain_id));
                }
            }
        }
        _ => {}
    }

    Ok(())
}

/// Rename a custom network
pub fn rename_network(config: &mut Config, old_name: &str, new_name: &str) -> Result<()> {
    let display = DisplayManager::new();

    if PREBUILT_REQUESTOR_NETWORKS.contains(&old_name)
        || PREBUILT_PROVER_NETWORKS.contains(&old_name)
        || PREBUILT_REWARDS_NETWORKS.contains(&old_name)
    {
        bail!(
            "Cannot rename pre-built network '{}'. Only custom networks can be renamed.",
            old_name
        );
    }

    let new_name_exists = PREBUILT_REQUESTOR_NETWORKS.contains(&new_name)
        || PREBUILT_PROVER_NETWORKS.contains(&new_name)
        || PREBUILT_REWARDS_NETWORKS.contains(&new_name)
        || config.custom_markets.iter().any(|m| m.name == new_name)
        || config.custom_rewards.iter().any(|r| r.name == new_name);

    if new_name_exists {
        bail!("Network name '{}' already exists", new_name);
    }

    let mut found_in_markets = false;
    for market in &mut config.custom_markets {
        if market.name == old_name {
            market.name = new_name.to_string();
            found_in_markets = true;
            break;
        }
    }

    let mut found_in_rewards = false;
    for rewards in &mut config.custom_rewards {
        if rewards.name == old_name {
            rewards.name = new_name.to_string();
            found_in_rewards = true;
            break;
        }
    }

    if !found_in_markets && !found_in_rewards {
        bail!("Network '{}' not found", old_name);
    }

    let mut updated_modules = Vec::new();

    if let Some(ref mut requestor) = config.requestor {
        if requestor.network == old_name {
            requestor.network = new_name.to_string();
            updated_modules.push("requestor");
        }
    }

    if let Some(ref mut prover) = config.prover {
        if prover.network == old_name {
            prover.network = new_name.to_string();
            updated_modules.push("prover");
        }
    }

    if let Some(ref mut rewards) = config.rewards {
        if rewards.network == old_name {
            rewards.network = new_name.to_string();
            updated_modules.push("rewards");
        }
    }

    display.success(&format!("Renamed network '{}' to '{}'", old_name, new_name));

    for module in updated_modules {
        display.success(&format!("Updated active {} network to '{}'", module, new_name));
    }

    Ok(())
}

fn print_section_header(display: &DisplayManager, title: &str) {
    display.header(&format!("\n{}", title));
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::primitives::Address;

    #[test]
    fn test_create_minimal_custom_network() {
        let network = create_minimal_custom_network(1234);

        assert_eq!(network.name, "custom-1234");
        assert_eq!(network.chain_id, 1234);
        assert_eq!(network.boundless_market_address, Address::ZERO);
        assert_eq!(network.set_verifier_address, Address::ZERO);
        assert_eq!(network.verifier_router_address, None);
        assert_eq!(network.collateral_token_address, None);
        assert_eq!(network.order_stream_url, None);
    }

    #[test]
    fn test_create_minimal_custom_rewards() {
        let rewards = create_minimal_custom_rewards(5678);

        assert_eq!(rewards.name, "custom-5678");
        assert_eq!(rewards.chain_id, 5678);
        assert_eq!(rewards.zkc_address, Address::ZERO);
        assert_eq!(rewards.vezkc_address, Address::ZERO);
        assert_eq!(rewards.staking_rewards_address, Address::ZERO);
        assert_eq!(rewards.mining_accounting_address, Address::ZERO);
        assert_eq!(rewards.mining_mint_address, Address::ZERO);
    }

    #[test]
    fn test_generate_unique_custom_name_no_collision() {
        let config = Config::default();
        let name = generate_unique_custom_name(&config, "test");
        assert_eq!(name, "custom-test");
    }

    #[test]
    fn test_generate_unique_custom_name_with_collision() {
        let mut config = Config::default();

        let mut network = create_minimal_custom_network(1234);
        network.name = "custom-test".to_string();
        config.custom_markets.push(network);

        let name = generate_unique_custom_name(&config, "test");
        assert_eq!(name, "custom-test-1");
    }

    #[test]
    fn test_generate_unique_custom_name_multiple_collisions() {
        let mut config = Config::default();

        for i in 0..3 {
            let mut network = create_minimal_custom_network(1234 + i);
            network.name =
                if i == 0 { "custom-test".to_string() } else { format!("custom-test-{}", i) };
            config.custom_markets.push(network);
        }

        let name = generate_unique_custom_name(&config, "test");
        assert_eq!(name, "custom-test-3");
    }

    #[test]
    fn test_update_custom_market_addresses_not_found() {
        let mut config = Config::default();
        let setup = SetupInteractive {
            network: None,
            rpc_url: None,
            private_key: None,
            address: None,
            staking_private_key: None,
            staking_address: None,
            reward_private_key: None,
            reward_address: None,
            beacon_api_url: None,
            state_file: None,
            boundless_market_address: None,
            verifier_router_address: None,
            set_verifier_address: None,
            collateral_token_address: None,
            order_stream_url: None,
            zkc_address: None,
            vezkc_address: None,
            staking_rewards_address: None,
            mining_accounting_address: None,
            mining_mint_address: None,
            rename_network: None,
            reset: false,
            reset_all: false,
        };

        let result = update_custom_market_addresses(&mut config, "nonexistent", &setup);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not found"));
    }

    #[test]
    fn test_update_custom_market_addresses_success() {
        let mut config = Config::default();
        config.custom_markets.push(create_minimal_custom_network(1234));

        let test_address = "0x1234567890123456789012345678901234567890";
        let setup = SetupInteractive {
            network: None,
            rpc_url: None,
            private_key: None,
            address: None,
            staking_private_key: None,
            staking_address: None,
            reward_private_key: None,
            reward_address: None,
            beacon_api_url: None,
            state_file: None,
            boundless_market_address: Some(test_address.to_string()),
            verifier_router_address: None,
            set_verifier_address: Some(test_address.to_string()),
            collateral_token_address: None,
            order_stream_url: Some("wss://test.com".to_string()),
            zkc_address: None,
            vezkc_address: None,
            staking_rewards_address: None,
            mining_accounting_address: None,
            mining_mint_address: None,
            rename_network: None,
            reset: false,
            reset_all: false,
        };

        let result = update_custom_market_addresses(&mut config, "custom-1234", &setup);
        assert!(result.is_ok());
        assert!(result.unwrap());

        let network = &config.custom_markets[0];
        assert_eq!(format!("{:#x}", network.boundless_market_address), test_address);
        assert_eq!(format!("{:#x}", network.set_verifier_address), test_address);
        assert_eq!(network.order_stream_url.as_ref().unwrap(), "wss://test.com");
    }

    #[test]
    fn test_update_custom_market_addresses_no_changes() {
        let mut config = Config::default();
        config.custom_markets.push(create_minimal_custom_network(1234));

        let setup = SetupInteractive {
            network: None,
            rpc_url: None,
            private_key: None,
            address: None,
            staking_private_key: None,
            staking_address: None,
            reward_private_key: None,
            reward_address: None,
            beacon_api_url: None,
            state_file: None,
            boundless_market_address: None,
            verifier_router_address: None,
            set_verifier_address: None,
            collateral_token_address: None,
            order_stream_url: None,
            zkc_address: None,
            vezkc_address: None,
            staking_rewards_address: None,
            mining_accounting_address: None,
            mining_mint_address: None,
            rename_network: None,
            reset: false,
            reset_all: false,
        };

        let result = update_custom_market_addresses(&mut config, "custom-1234", &setup);
        assert!(result.is_ok());
        assert!(!result.unwrap());
    }

    #[test]
    fn test_update_custom_rewards_addresses_success() {
        let mut config = Config::default();
        config.custom_rewards.push(create_minimal_custom_rewards(5678));

        let test_address = "0x1234567890123456789012345678901234567890";
        let setup = SetupInteractive {
            network: None,
            rpc_url: None,
            private_key: None,
            address: None,
            staking_private_key: None,
            staking_address: None,
            reward_private_key: None,
            reward_address: None,
            beacon_api_url: None,
            state_file: None,
            boundless_market_address: None,
            verifier_router_address: None,
            set_verifier_address: None,
            collateral_token_address: None,
            order_stream_url: None,
            zkc_address: Some(test_address.to_string()),
            vezkc_address: Some(test_address.to_string()),
            staking_rewards_address: Some(test_address.to_string()),
            mining_accounting_address: Some(test_address.to_string()),
            mining_mint_address: Some(test_address.to_string()),
            rename_network: None,
            reset: false,
            reset_all: false,
        };

        let result = update_custom_rewards_addresses(&mut config, "custom-5678", &setup);
        assert!(result.unwrap());

        let rewards = &config.custom_rewards[0];
        assert_eq!(format!("{:#x}", rewards.zkc_address), test_address);
        assert_eq!(format!("{:#x}", rewards.vezkc_address), test_address);
        assert_eq!(format!("{:#x}", rewards.staking_rewards_address), test_address);
        assert_eq!(format!("{:#x}", rewards.mining_accounting_address), test_address);
        assert_eq!(format!("{:#x}", rewards.mining_mint_address), test_address);
    }

    #[test]
    fn test_rename_network_prebuilt_fails() {
        let mut config = Config::default();
        let result = rename_network(&mut config, "base-mainnet", "my-network");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("pre-built"));
    }

    #[test]
    fn test_rename_network_collision_fails() {
        let mut config = Config::default();

        let mut network1 = create_minimal_custom_network(1234);
        network1.name = "custom-1".to_string();
        config.custom_markets.push(network1);

        let mut network2 = create_minimal_custom_network(5678);
        network2.name = "custom-2".to_string();
        config.custom_markets.push(network2);

        let result = rename_network(&mut config, "custom-1", "custom-2");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("already exists"));
    }

    #[test]
    fn test_rename_network_success() {
        let mut config = Config::default();

        let mut network = create_minimal_custom_network(1234);
        network.name = "old-name".to_string();
        config.custom_markets.push(network);

        let result = rename_network(&mut config, "old-name", "new-name");
        assert!(result.is_ok());
        assert_eq!(config.custom_markets[0].name, "new-name");
    }

    #[test]
    fn test_rename_network_not_found() {
        let mut config = Config::default();
        let result = rename_network(&mut config, "nonexistent", "new-name");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not found"));
    }
}
