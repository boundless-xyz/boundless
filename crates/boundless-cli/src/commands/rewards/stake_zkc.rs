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

use alloy::{
    network::Ethereum,
    primitives::{utils::format_ether, U256},
    providers::{PendingTransactionBuilder, Provider, ProviderBuilder},
    signers::Signer,
};
use anyhow::{Context, Result};
use boundless_market::contracts::token::{IERC20Permit, Permit};
use boundless_zkc::{
    contracts::{extract_tx_log, DecodeRevert, IStaking},
    deployments::Deployment,
};
use clap::Args;

use crate::{
    config::GlobalConfig,
    indexer_client::{parse_amount, IndexerClient},
};

/// Stake ZKC tokens with optional dry-run to estimate rewards
#[derive(Args, Clone, Debug)]
pub struct RewardsStakeZkc {
    /// Amount of ZKC to stake, in wei.
    #[clap(long)]
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

    /// Configuration for the ZKC deployment to use.
    #[clap(flatten, next_help_heading = "ZKC Deployment")]
    pub deployment: Option<Deployment>,
}

impl RewardsStakeZkc {
    /// Run the stake-zkc command
    pub async fn run(&self, global_config: &GlobalConfig) -> Result<()> {
        if self.dry_run {
            return self.run_dry_run(global_config).await;
        }

        // Actual staking implementation (migrated from zkc/stake.rs)
        let tx_signer = global_config.require_private_key()?;
        let rpc_url = global_config.require_rpc_url()?;

        // Connect to the chain
        let provider = ProviderBuilder::new()
            .wallet(tx_signer.clone())
            .connect(rpc_url.as_str())
            .await
            .with_context(|| format!("failed to connect provider to {rpc_url}"))?;
        let chain_id = provider.get_chain_id().await?;
        let deployment = self.deployment.clone().or_else(|| Deployment::from_chain_id(chain_id))
            .context("could not determine ZKC deployment from chain ID; please specify deployment explicitly")?;

        let token_id =
            get_active_token_id(provider.clone(), deployment.vezkc_address, tx_signer.address())
                .await?;
        let add = !token_id.is_zero();

        let pending_tx = match self.no_permit {
            false => {
                self.stake_with_permit(
                    provider,
                    deployment,
                    self.amount,
                    &tx_signer,
                    self.permit_deadline,
                    add,
                )
                .await?
            }
            true => self
                .stake(provider, deployment, self.amount, add)
                .await
                .context("Sending stake transaction failed")?,
        };

        tracing::debug!("Broadcasting stake deposit tx {}", pending_tx.tx_hash());
        let tx_hash = pending_tx.tx_hash();
        tracing::info!(%tx_hash, "Sent transaction for staking");

        let receipt = pending_tx
            .with_required_confirmations(1)
            .get_receipt()
            .await
            .context("failed to get transaction receipt")?;
        // Extract the appropriate log based on whether we're adding or creating
        let (token_id, stake_amount) = if add {
            let log = extract_tx_log::<IStaking::StakeAdded>(&receipt)?;
            (U256::from(log.data().tokenId), log.data().newTotal)
        } else {
            let log = extract_tx_log::<IStaking::StakeCreated>(&receipt)?;
            (U256::from(log.data().tokenId), log.data().amount)
        };

        tracing::info!(
            "Successfully staked {} ZKC with token ID {}",
            format_ether(stake_amount),
            token_id
        );

        Ok(())
    }

    async fn run_dry_run(&self, global_config: &GlobalConfig) -> Result<()> {
        tracing::info!("=== DRY RUN MODE: Estimating staking rewards ===");
        tracing::warn!("⚠️  These are estimates only. Actual rewards depend on:");
        tracing::warn!("   - Total staked amount in the epoch");
        tracing::warn!("   - Your staking duration");
        tracing::warn!("   - Network activity and emissions");

        // Get chain ID from ETH_MAINNET_RPC_URL to determine indexer endpoint
        let rpc_url = global_config.require_rpc_url()?;
        let provider = ProviderBuilder::new()
            .connect(rpc_url.as_str())
            .await
            .with_context(|| format!("failed to connect provider to {rpc_url}"))?;
        let chain_id = provider.get_chain_id().await?;

        // Create indexer client based on chain ID
        let client = IndexerClient::new_from_chain_id(chain_id)?;

        // Get current epoch from contract (would need ETH_MAINNET_RPC_URL)
        // For now, we'll estimate based on recent epochs
        // TODO: Query actual current epoch from contract
        let estimated_epoch = 5; // Placeholder

        tracing::info!("\nFetching current epoch staking data...");
        let epoch_data = client.get_current_epoch_staking(estimated_epoch).await?;

        if let Some(summary) = &epoch_data.summary {
            let total_staked = parse_amount(&summary.total_staked)?;
            let your_stake = self.amount;
            let new_total = total_staked + your_stake;

            tracing::info!("\n=== Current Epoch {} Statistics ===", summary.epoch);
            tracing::info!("Total Staked: {} ZKC", format_ether(total_staked));
            tracing::info!("Number of Stakers: {}", summary.num_stakers);
            tracing::info!("Your Planned Stake: {} ZKC", format_ether(your_stake));

            // Calculate percentage share
            // Calculate percentage share
            let your_percentage = if new_total > U256::ZERO {
                // Rough percentage calculation
                (your_stake * U256::from(10000) / new_total).to::<u64>() as f64 / 100.0
            } else {
                100.0
            };

            tracing::info!("\n=== Your Estimated Position ===");
            tracing::info!("Your Share: {:.2}% of total stake", your_percentage);
            tracing::info!("New Total Staked: {} ZKC", format_ether(new_total));

            // Rough reward estimate (would need actual emission rates)
            // This is a placeholder calculation
            let estimated_epoch_rewards =
                U256::from(1000000_u64) * U256::from(10).pow(U256::from(18));
            let your_estimated_rewards = estimated_epoch_rewards * your_stake / new_total;

            tracing::info!("\n=== Estimated Rewards (Per Epoch) ===");
            tracing::info!("Estimated Rewards: ~{} ZKC", format_ether(your_estimated_rewards));
            // Calculate APY as a rough estimate
            let apy = if your_stake > U256::ZERO {
                let reward_ratio = (your_estimated_rewards * U256::from(10000) / your_stake)
                    .to::<u64>() as f64
                    / 10000.0;
                reward_ratio * 52.0 * 100.0
            } else {
                0.0
            };
            tracing::info!("Annual Rate: ~{:.2}% APY (assuming consistent conditions)", apy);

            tracing::warn!("\n⚠️  IMPORTANT DISCLAIMERS:");
            tracing::warn!("   - These estimates assume current epoch conditions remain constant");
            tracing::warn!("   - Actual rewards vary based on network participation");
            tracing::warn!("   - Staking locks your tokens for a minimum period");
            tracing::warn!("   - Early withdrawal may incur penalties");
        } else {
            tracing::warn!("Unable to fetch current epoch data for estimation");
        }

        tracing::info!("\nTo proceed with actual staking, run without --dry-run flag");

        Ok(())
    }

    async fn stake_with_permit<P>(
        &self,
        provider: P,
        deployment: Deployment,
        amount: U256,
        signer: &impl Signer,
        deadline: u64,
        add: bool,
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

        tracing::info!("Staking {} ZKC with permit", format_ether(amount));

        let send_result = if add {
            staking.addToStakeWithPermit(amount, U256::from(deadline), v, r, s).send().await
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
        add: bool,
    ) -> Result<PendingTransactionBuilder<Ethereum>>
    where
        P: Provider<Ethereum> + Clone,
    {
        let staking = IStaking::new(deployment.vezkc_address, provider);

        tracing::info!("Staking {} ZKC", format_ether(amount));

        let send_result = if add {
            staking.addToStake(amount).send().await
        } else {
            staking.stake(amount).send().await
        };

        send_result
            .maybe_decode_revert::<IStaking::IStakingErrors>()
            .context("Failed to send stake transaction")
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
