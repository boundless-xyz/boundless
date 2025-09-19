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

use std::io::{self, Write};

use alloy::{
    eips::BlockId,
    network::Ethereum,
    primitives::{
        utils::{format_ether, parse_units},
        Address, B256, U256,
    },
    providers::{PendingTransactionBuilder, Provider, ProviderBuilder},
    signers::Signer,
    sol_types::SolCall,
};
use anyhow::{anyhow, bail, ensure, Context};
use boundless_market::contracts::token::{IERC20Permit, Permit, IERC20};
use boundless_zkc::{
    contracts::{extract_tx_log, DecodeRevert, IStaking},
    deployments::Deployment,
};
use clap::Args;

use crate::{commands::zkc::get_active_token_id, config::GlobalConfig};

/// Command to stake ZKC.
#[non_exhaustive]
#[derive(Args, Clone, Debug)]
pub struct ZkcStake {
    /// Amount of ZKC to stake.
    ///
    /// This is specified in ZKC, e.g., to stake 1 ZKC, use `--amount 1`.
    #[clap(long)]
    amount: String,
    /// Do not use ERC20 permit to authorize the staking. You will need to send a separate
    /// transaction to set an ERC20 allowance instead.
    #[clap(long)]
    pub no_permit: bool,
    /// Deadline for the ERC20 permit, in seconds.
    #[clap(long, default_value_t = 3600, conflicts_with = "no_permit")]
    pub permit_deadline: u64,
    /// Whether to only print the calldata without sending the transaction.
    #[clap(long)]
    pub calldata: bool,
    /// The account address to stake from.
    ///
    /// Only valid when used with `--calldata`.
    #[clap(long, requires = "calldata")]
    pub from: Option<Address>,
    /// Configuration for the ZKC deployment to use.
    #[clap(flatten, next_help_heading = "ZKC Deployment")]
    pub deployment: Option<Deployment>,
}

impl ZkcStake {
    /// Run the [ZKCStake] command.
    pub async fn run(&self, global_config: &GlobalConfig) -> anyhow::Result<()> {
        let rpc_url = global_config.require_rpc_url()?;

        // Connect to the chain.
        let provider = ProviderBuilder::new()
            .connect(rpc_url.as_str())
            .await
            .with_context(|| format!("failed to connect provider to {rpc_url}"))?;
        let chain_id = provider.get_chain_id().await?;
        let deployment = self.deployment.clone().or_else(|| Deployment::from_chain_id(chain_id))
            .context("could not determine ZKC deployment from chain ID; please specify deployment explicitly")?;

        let account = match &self.from {
            Some(addr) => *addr,
            None => global_config.require_private_key()?.address(),
        };

        let token_id =
            get_active_token_id(provider.clone(), deployment.vezkc_address, account).await?;
        let add = !token_id.is_zero();

        let parsed_amount = parse_units(&self.amount, 18)
            .map_err(|e| anyhow!("Failed to parse ZKC amount: {}", e))?
            .into();
        if parsed_amount == U256::from(0) {
            bail!("Amount is below the denomination minimum: {}", self.amount);
        }

        if self.calldata {
            return self.approve_then_stake(deployment, parsed_amount, add).await;
        }

        let tx_signer = global_config.require_private_key()?;
        let provider = ProviderBuilder::new()
            .wallet(tx_signer.clone())
            .connect(rpc_url.as_str())
            .await
            .with_context(|| format!("failed to connect provider to {rpc_url}"))?;

        if !add {
            println!(
                "You're creating a new ZKC stake position. This will lock {} ZKC for 30 days.",
                format_ether(parsed_amount)
            );
            print!("Type 'yes' to confirm and continue: ");
            io::stdout().flush().ok();
            let mut input = String::new();
            io::stdin()
                .read_line(&mut input)
                .map_err(|e| anyhow!("failed to read confirmation: {}", e))?;
            if input.trim().to_lowercase() != "yes" {
                bail!("Stake cancelled by user");
            }
        }

        let pending_tx = match self.no_permit {
            false => {
                self.stake_with_permit(
                    provider,
                    deployment,
                    parsed_amount,
                    &tx_signer,
                    self.permit_deadline,
                    add,
                )
                .await?
            }
            true => self
                .stake(provider, deployment, parsed_amount, add)
                .await
                .context("Sending stake transaction failed")?,
        };
        tracing::debug!("Broadcasting stake deposit tx {}", pending_tx.tx_hash());
        let tx_hash = pending_tx.tx_hash();
        tracing::info!(%tx_hash, "Sent transaction for staking");

        let timeout = global_config.tx_timeout.or(pending_tx.timeout());

        tracing::debug!(?timeout, %tx_hash, "Waiting for transaction receipt");
        let tx_receipt = pending_tx
            .with_timeout(timeout)
            .get_receipt()
            .await
            .context("Failed to receive receipt staking transaction")?;

        ensure!(
            tx_receipt.status(),
            "Staking transaction failed: tx_hash = {}",
            tx_receipt.transaction_hash
        );

        if add {
            let (token_id, owner, amount_added, new_total) =
                match extract_tx_log::<IStaking::StakeAdded>(&tx_receipt) {
                    Ok(log) => (
                        U256::from(log.data().tokenId),
                        log.data().owner,
                        log.data().addedAmount,
                        log.data().newTotal,
                    ),
                    Err(e) => anyhow::bail!("Failed to extract stake created log: {}", e),
                };
            tracing::info!(
                "Staking completed: token_id = {token_id}, owner = {owner}, amount added = {} ZKC, new total = {} ZKC",
                format_ether(amount_added),
                format_ether(new_total)
            );
        } else {
            let (token_id, owner, amount) =
                match extract_tx_log::<IStaking::StakeCreated>(&tx_receipt) {
                    Ok(log) => {
                        (U256::from(log.data().tokenId), log.data().owner, log.data().amount)
                    }
                    Err(e) => anyhow::bail!("Failed to extract stake created log: {}", e),
                };
            tracing::info!(
                "Staking completed: token_id = {token_id}, owner = {owner}, amount = {} ZKC",
                format_ether(amount)
            );
        }
        Ok(())
    }

    async fn approve_then_stake(
        &self,
        deployment: Deployment,
        value: U256,
        add: bool,
    ) -> anyhow::Result<()> {
        let approve_call = IERC20::approveCall { spender: deployment.vezkc_address, value };
        println!("========= Approve Call =========");
        println!("target address: {}", deployment.zkc_address);
        println!("calldata: 0x{}", hex::encode(approve_call.abi_encode()));

        println!("========= Staking Call =========");
        println!("target address: {}", deployment.vezkc_address);
        let staking_call = if add {
            IStaking::addToStakeCall { amount: value }.abi_encode()
        } else {
            IStaking::stakeCall { amount: value }.abi_encode()
        };
        println!("calldata: 0x{}", hex::encode(staking_call));
        Ok(())
    }

    async fn stake(
        &self,
        provider: impl Provider + Clone,
        deployment: Deployment,
        value: U256,
        add: bool,
    ) -> Result<PendingTransactionBuilder<Ethereum>, anyhow::Error> {
        let staking = IStaking::new(deployment.vezkc_address, provider);
        let send_result = match add {
            false => {
                tracing::trace!("Calling stake({})", value);
                staking.stake(value).send().await
            }
            true => {
                tracing::trace!("Calling addToStake({})", value);
                staking.addToStake(value).send().await
            }
        };
        send_result
            .maybe_decode_revert::<IStaking::IStakingErrors>()
            .context("Sending stake transaction failed")
    }

    async fn stake_with_permit(
        &self,
        provider: impl Provider + Clone,
        deployment: Deployment,
        value: U256,
        signer: &impl Signer,
        deadline: u64,
        add: bool,
    ) -> Result<PendingTransactionBuilder<Ethereum>, anyhow::Error> {
        let contract = IERC20Permit::new(deployment.zkc_address, provider.clone());
        let owner = signer.address();
        let call = contract.nonces(owner);
        // TODO(zkc): Map to proper error
        let nonce = call.call().await.map_err(|e| anyhow::anyhow!("Failed to get nonce: {}", e))?;

        // Compute the deadline for the permit using the latest block.
        let latest_block = provider
            .get_block(BlockId::latest())
            .await
            .context("Failed to check the current block timestamp")?
            .context("Latest block response is empty")?;
        let deadline = U256::from(deadline + latest_block.header.timestamp);

        // Build and sign a permit
        let permit = Permit { owner, spender: deployment.vezkc_address, value, nonce, deadline };
        tracing::debug!("Permit: {:?}", permit);
        let domain_separator = contract.DOMAIN_SEPARATOR().call().await?;
        let sig = permit.sign(signer, domain_separator).await?.as_bytes();
        let r = B256::from_slice(&sig[..32]);
        let s = B256::from_slice(&sig[32..64]);
        let v: u8 = sig[64];

        let staking = IStaking::new(deployment.vezkc_address, provider);
        let send_result = match add {
            false => {
                tracing::trace!("Calling stakeWithPermit({})", value);
                staking.stakeWithPermit(value, deadline, v, r, s).send().await
            }
            true => {
                tracing::trace!("Calling addToStakeWithPermit({})", value);
                staking.addToStakeWithPermit(value, deadline, v, r, s).send().await
            }
        };
        send_result
            .maybe_decode_revert::<IStaking::IStakingErrors>()
            .context("Sending stake with permit transaction failed")
    }
}
