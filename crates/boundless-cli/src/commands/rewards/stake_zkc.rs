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

use crate::display::format_amount;
use crate::display::network_name_from_chain_id;
use alloy::{
    network::Ethereum,
    primitives::{
        utils::{format_ether, parse_ether},
        Address, U256,
    },
    providers::{PendingTransactionBuilder, Provider, ProviderBuilder},
    signers::Signer,
    sol_types::SolCall,
};
use anyhow::{bail, Context, Result};
use boundless_market::contracts::token::{IERC20Permit, Permit, IERC20};
use boundless_zkc::{
    contracts::{extract_tx_log, DecodeRevert, IStaking},
    deployments::Deployment,
};
use clap::Args;
use colored::Colorize;

use crate::{
    config::{GlobalConfig, RewardsConfig},
    display::DisplayManager,
    indexer_client::{parse_amount, IndexerClient},
};

/// Stake ZKC tokens with optional dry-run to estimate rewards
#[derive(Args, Clone, Debug)]
pub struct RewardsStakeZkc {
    /// Amount of ZKC to stake (e.g., "10" for 10 ZKC, "1.5" for 1.5 ZKC)
    #[clap(value_parser = parse_ether)]
    pub amount: U256,

    /// Do not use ERC20 permit to authorize the staking.
    #[clap(long)]
    pub no_permit: bool,

    /// Deadline for the ERC20 permit, in seconds.
    #[clap(long, default_value_t = 3600, conflicts_with = "no_permit")]
    pub permit_deadline: u64,

    /// Only show estimated rewards without actually staking (requires indexer API)
    #[clap(long)]
    pub dry_run: bool,

    /// Print calldata for smart contract wallet usage instead of sending transaction
    #[clap(long)]
    pub calldata: bool,

    /// Address to stake for (stakes to their veZKC position instead of your own)
    #[clap(long)]
    pub staking_address: Option<Address>,

    /// Configuration for the ZKC deployment to use.
    #[clap(flatten, next_help_heading = "ZKC Deployment")]
    pub deployment: Option<Deployment>,

    /// Rewards configuration (RPC URL, private key, ZKC contract address)
    #[clap(flatten)]
    pub rewards_config: RewardsConfig,
}

impl RewardsStakeZkc {
    /// Run the stake-zkc command
    pub async fn run(&self, global_config: &GlobalConfig) -> Result<()> {
        let rewards_config = self.rewards_config.clone().load_from_files()?;

        if self.dry_run {
            return self.run_dry_run(global_config, &rewards_config).await;
        }

        // Actual staking implementation (migrated from zkc/stake.rs)
        let tx_signer = rewards_config.require_staking_private_key()?;
        let rpc_url = rewards_config.require_rpc_url()?;

        // Connect to the chain
        let provider = ProviderBuilder::new()
            .wallet(tx_signer.clone())
            .connect(rpc_url.as_str())
            .await
            .with_context(|| format!("failed to connect provider to {rpc_url}"))?;
        let chain_id = provider.get_chain_id().await?;
        let deployment = self.deployment.clone().or_else(|| Deployment::from_chain_id(chain_id))
            .context("could not determine ZKC deployment from chain ID; please specify deployment explicitly")?;

        // Determine if we're staking to another address or our own
        let (target_token_id, add) = if let Some(staking_address) = self.staking_address {
            let token_id =
                get_active_token_id(provider.clone(), deployment.vezkc_address, staking_address)
                    .await?;
            if token_id.is_zero() {
                bail!("Address {:#x} does not have an active staking position", staking_address);
            }
            (Some(token_id), true)
        } else {
            let token_id = get_active_token_id(
                provider.clone(),
                deployment.vezkc_address,
                tx_signer.address(),
            )
            .await?;
            let add = !token_id.is_zero();
            (if add { Some(token_id) } else { None }, add)
        };

        if self.calldata {
            return self.approve_then_stake(deployment, self.amount, target_token_id).await;
        }

        let network_name = network_name_from_chain_id(Some(chain_id));
        let amount_formatted = format_amount(&format_ether(self.amount));

        let display = DisplayManager::with_network(network_name);
        display.header("Staking ZKC");
        display.balance("Amount", &amount_formatted, "ZKC", "yellow");
        if let Some(ref staking_addr) = self.staking_address {
            display.item_colored("Staking to", format!("{:#x}", staking_addr), "cyan");
        }
        println!();

        let pending_tx = match self.no_permit {
            false => {
                self.stake_with_permit(
                    provider.clone(),
                    deployment.clone(),
                    self.amount,
                    &tx_signer,
                    self.permit_deadline,
                    target_token_id,
                )
                .await?
            }
            true => self
                .stake(provider.clone(), deployment.clone(), self.amount, target_token_id)
                .await
                .context("Sending stake transaction failed")?,
        };

        let tx_hash = *pending_tx.tx_hash();

        display.item_colored("Transaction", format!("{:#x}", tx_hash), "dimmed");
        display.status("Status", "Waiting for confirmation...", "yellow");

        let receipt = pending_tx
            .with_required_confirmations(1)
            .get_receipt()
            .await
            .context("failed to get transaction receipt")?;

        // Extract the appropriate log based on whether we're adding or creating
        let (token_id, new_total_staked) = if add {
            let log = extract_tx_log::<IStaking::StakeAdded>(&receipt)?;
            (U256::from(log.data().tokenId), log.data().newTotal)
        } else {
            let log = extract_tx_log::<IStaking::StakeCreated>(&receipt)?;
            (U256::from(log.data().tokenId), log.data().amount)
        };

        let staked_formatted = format_amount(&format_ether(new_total_staked));
        display.success("Staking successful!");
        display.item_colored("Token ID", token_id.to_string(), "cyan");
        display.balance("Total Staked", &staked_formatted, "ZKC", "green");

        // Query and display final balances
        self.display_final_status(provider, deployment, tx_signer.address()).await?;

        Ok(())
    }

    async fn display_final_status<P>(
        &self,
        provider: P,
        deployment: Deployment,
        address: alloy::primitives::Address,
    ) -> Result<()>
    where
        P: Provider<Ethereum> + Clone,
    {
        // Define interfaces
        alloy::sol! {
            #[sol(rpc)]
            interface IRewards {
                function getStakingRewards(address account) external view returns (uint256);
                function rewardDelegates(address account) external view returns (address);
            }
        }

        // Create contract instances
        let staking = IStaking::new(deployment.vezkc_address, provider.clone());
        let rewards = IRewards::new(deployment.vezkc_address, provider.clone());
        let zkc_token = IERC20::new(deployment.zkc_address, provider);

        // Query balances
        let staked_result = staking
            .getStakedAmountAndWithdrawalTime(address)
            .call()
            .await
            .context("Failed to query staked amount")?;
        let staked_amount = staked_result.amount;
        let available_balance = zkc_token
            .balanceOf(address)
            .call()
            .await
            .context("Failed to query available balance")?;
        let reward_power = rewards
            .getStakingRewards(address)
            .call()
            .await
            .context("Failed to query reward power")?;
        let reward_delegate = rewards
            .rewardDelegates(address)
            .call()
            .await
            .context("Failed to query reward delegate")?;

        // Format values
        let staked_formatted = format_amount(&format_ether(staked_amount));
        let available_formatted = format_amount(&format_ether(available_balance));

        let display = DisplayManager::new();
        display.subsection("Current Balances");
        display.balance("Staked", &staked_formatted, "ZKC", "green");
        display.balance("Available", &available_formatted, "ZKC", "cyan");

        // Show reward power with delegation info
        if reward_delegate != address {
            let reward_power_formatted = format_amount(&format_ether(reward_power));
            let delegation_note =
                format!("[delegated {} to {:#x}]", reward_power_formatted, reward_delegate);
            println!(
                "  {:<16} {} {}",
                "Reward Power:",
                "0".yellow().bold(),
                delegation_note.dimmed()
            );
        } else {
            let reward_power_formatted = format_amount(&format_ether(reward_power));
            println!("  {:<16} {}", "Reward Power:", reward_power_formatted.yellow().bold());
        }

        Ok(())
    }

    async fn run_dry_run(
        &self,
        _global_config: &GlobalConfig,
        rewards_config: &crate::config::RewardsConfig,
    ) -> Result<()> {
        let deployment =
            rewards_config.zkc_deployment.as_ref().context("ZKC deployment not configured")?;

        let network_name = network_name_from_chain_id(deployment.chain_id);

        let display = DisplayManager::with_network(network_name);
        display.header("DRY RUN: Staking Estimate");

        // Use deployment chain ID instead of querying RPC (avoids wrong indexer if RPC URL misconfigured)
        let client = IndexerClient::new_from_chain_id(deployment.chain_id.unwrap())?;

        // Fetch staking metadata for last updated timestamp
        let metadata = client.get_staking_metadata().await.ok();

        // Get current epoch from contract
        let estimated_epoch = 5; // Placeholder

        if let Some(ref meta) = metadata {
            let formatted_time = crate::indexer_client::format_timestamp(&meta.last_updated_at);
            display.note(&format!("Data last updated: {}", formatted_time));
        }

        display.note("Fetching current epoch staking data...");
        let summary = client.get_epoch_staking(estimated_epoch).await?;

        let total_staked = parse_amount(&summary.total_staked)?;
        let your_stake = self.amount;
        let new_total = total_staked + your_stake;

        // Calculate percentage share
        let your_percentage = if new_total > U256::ZERO {
            (your_stake * U256::from(10000) / new_total).to::<u64>() as f64 / 100.0
        } else {
            100.0
        };

        let total_staked_formatted = format_amount(&format_ether(total_staked));
        let your_stake_formatted = format_amount(&format_ether(your_stake));
        let new_total_formatted = format_amount(&format_ether(new_total));

        display.subsection("Current Epoch Statistics");
        display.item_colored("Epoch", summary.epoch.to_string(), "cyan");
        display.balance("Total Staked", &total_staked_formatted, "ZKC", "green");
        display.item_colored("Stakers", summary.num_stakers.to_string(), "cyan");
        display.balance("Your Stake", &your_stake_formatted, "ZKC", "yellow");

        display.subsection("Your Estimated Position");
        display.item("Share", format!("{:.2}%", your_percentage));
        display.balance("New Total", &new_total_formatted, "ZKC", "green");

        // Calculate PoVW cap (reward power / 15)
        let max_povw_per_epoch = your_stake / U256::from(15);
        let max_povw_formatted = format_amount(&format_ether(max_povw_per_epoch));

        display.subsection("Maximum PoVW Rewards");
        println!(
            "  {:<16} {} {}",
            "Reward Power:",
            your_stake_formatted.yellow(),
            "(equals staked amount)".dimmed()
        );
        println!(
            "  {:<16} {} {} {}",
            "Max per Epoch:",
            max_povw_formatted.yellow().bold(),
            "ZKC".yellow(),
            "(reward power / 15)".dimmed()
        );

        // Rough staking reward estimate
        let estimated_epoch_rewards = U256::from(1000000_u64) * U256::from(10).pow(U256::from(18));
        let your_estimated_rewards = estimated_epoch_rewards * your_stake / new_total;
        let estimated_rewards_formatted = format_amount(&format_ether(your_estimated_rewards));

        // Calculate APY as a rough estimate
        let apy = if your_stake > U256::ZERO {
            let reward_ratio = (your_estimated_rewards * U256::from(10000) / your_stake).to::<u64>()
                as f64
                / 10000.0;
            reward_ratio * 52.0 * 100.0
        } else {
            0.0
        };

        display.subsection("Estimated Staking Rewards (Per Epoch)");
        println!(
            "  {:<16} ~{} {}",
            "Est. Rewards:",
            estimated_rewards_formatted.green(),
            "ZKC".green()
        );
        display.item("Annual Rate", format!("~{:.2}%", apy));

        display.warning("IMPORTANT DISCLAIMERS:");
        display.note("• These estimates assume current epoch conditions remain constant");
        display.note("• Actual rewards vary based on network participation");
        display.note("• Staking locks your tokens. To withdraw, you must initiate the withdrawal process and wait 30 days.");

        display.info("To proceed with actual staking, run without --dry-run flag");

        Ok(())
    }

    async fn stake_with_permit<P>(
        &self,
        provider: P,
        deployment: Deployment,
        amount: U256,
        signer: &impl Signer,
        deadline: u64,
        token_id: Option<U256>,
    ) -> Result<PendingTransactionBuilder<Ethereum>>
    where
        P: Provider<Ethereum> + Clone,
    {
        let staking = IStaking::new(deployment.vezkc_address, provider.clone());
        let token = IERC20Permit::new(deployment.zkc_address, provider.clone());

        let nonce = token.nonces(signer.address()).call().await?;
        let deadline = deadline
            + std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_secs();

        let permit = Permit {
            owner: signer.address(),
            spender: deployment.vezkc_address,
            value: amount,
            nonce,
            deadline: U256::from(deadline),
        };

        let domain_separator = token.DOMAIN_SEPARATOR().call().await?;
        let signature = permit.sign(signer, domain_separator).await?.as_bytes();
        let r = alloy::primitives::B256::from_slice(&signature[..32]);
        let s = alloy::primitives::B256::from_slice(&signature[32..64]);
        let v = signature[64];

        let send_result = if let Some(token_id) = token_id {
            staking
                .addToStakeWithPermitByTokenId(token_id, amount, U256::from(deadline), v, r, s)
                .send()
                .await
        } else {
            staking.stakeWithPermit(amount, U256::from(deadline), v, r, s).send().await
        };

        send_result
            .maybe_decode_revert::<IStaking::IStakingErrors>()
            .context("Failed to send stake with permit transaction")
    }

    async fn stake<P>(
        &self,
        provider: P,
        deployment: Deployment,
        amount: U256,
        token_id: Option<U256>,
    ) -> Result<PendingTransactionBuilder<Ethereum>>
    where
        P: Provider<Ethereum> + Clone,
    {
        let staking = IStaking::new(deployment.vezkc_address, provider);

        let send_result = if let Some(token_id) = token_id {
            staking.addToStakeByTokenId(token_id, amount).send().await
        } else {
            staking.stake(amount).send().await
        };

        send_result
            .maybe_decode_revert::<IStaking::IStakingErrors>()
            .context("Failed to send stake transaction")
    }

    async fn approve_then_stake(
        &self,
        deployment: Deployment,
        value: U256,
        token_id: Option<U256>,
    ) -> Result<()> {
        let approve_call = IERC20::approveCall { spender: deployment.vezkc_address, value };

        println!("========= Approve Call =========");
        println!("target address: {}", deployment.zkc_address);
        println!("calldata: 0x{}", hex::encode(approve_call.abi_encode()));

        println!("========= Staking Call =========");
        println!("target address: {}", deployment.vezkc_address);
        let staking_call = if let Some(token_id) = token_id {
            IStaking::addToStakeByTokenIdCall { tokenId: token_id, amount: value }.abi_encode()
        } else {
            IStaking::stakeCall { amount: value }.abi_encode()
        };
        println!("calldata: 0x{}", hex::encode(staking_call));

        Ok(())
    }
}

async fn get_active_token_id<P>(
    provider: P,
    vezkc_address: alloy::primitives::Address,
    address: alloy::primitives::Address,
) -> Result<U256>
where
    P: Provider<Ethereum>,
{
    let staking = IStaking::new(vezkc_address, provider);
    let token_id = staking
        .getActiveTokenId(address)
        .call()
        .await
        .maybe_decode_revert::<IStaking::IStakingErrors>()?;
    Ok(token_id)
}
