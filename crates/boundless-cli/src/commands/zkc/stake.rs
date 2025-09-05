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
    eips::BlockId,
    network::Ethereum,
    primitives::{Address, B256, U256},
    providers::{PendingTransactionBuilder, Provider, ProviderBuilder},
    signers::Signer,
};
use anyhow::{ensure, Context};
use boundless_market::contracts::token::{IERC20Permit, Permit};
use boundless_zkc::contracts::{extract_tx_log, IStaking};
use clap::Args;

use crate::config::GlobalConfig;

/// Command to stake ZKC.
#[non_exhaustive]
#[derive(Args, Clone, Debug)]
pub struct ZkcStake {
    /// Amount of ZKC to stake, in wei.
    #[clap(long)]
    pub amount: U256,
    /// Do not use ERC20 permit to authorize the staking. You will need to send a separate
    /// transaction to set an ERC20 allowance instead.
    #[clap(long)]
    pub no_permit: bool,
    // TODO(victor): Can we drop this flag and just call stake or addStake based on whether they
    // are already staked?
    /// Add to an existing staking position. If this flag is not specified, the there must be no
    /// staked tokens for the given address. If there are staked tokens, this flag must be
    /// specified.
    #[clap(long)]
    pub add: bool,
    /// Deadline for the ERC20 permit, in seconds.
    #[clap(long, default_value_t = 3600, conflicts_with = "no_permit")]
    pub permit_deadline: u64,
    /// Address of the [IStaking] contract.
    #[clap(long, env = "VEZKC_ADDRESS")]
    pub vezkc_address: Address,
    /// Address of the ZKC token to permit.
    #[clap(long, env = "ZKC_ADDRESS", required_unless_present = "no_permit")]
    pub zkc_address: Option<Address>,
}

#[derive(Args, Clone, Debug)]
/// Parameters for permit-based staking.
pub struct WithPermit {}

impl ZkcStake {
    /// Run the [ZKCStake] command.
    pub async fn run(&self, global_config: &GlobalConfig) -> anyhow::Result<()> {
        let tx_signer = global_config.require_private_key()?;
        let rpc_url = global_config.require_rpc_url()?;

        // Connect to the chain.
        let provider = ProviderBuilder::new()
            .wallet(tx_signer.clone())
            .connect(rpc_url.as_str())
            .await
            .with_context(|| format!("failed to connect provider to {rpc_url}"))?;

        let pending_tx = match self.no_permit {
            false => {
                self.stake_with_permit(
                    provider,
                    self.zkc_address.context("ZKC contract address is required")?,
                    self.amount,
                    &tx_signer,
                    self.permit_deadline,
                    self.add,
                )
                .await?
            }
            true => self
                .stake(provider, self.amount, self.add)
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

        if self.add {
            let (token_id, owner, amount) =
                match extract_tx_log::<IStaking::StakeCreated>(&tx_receipt) {
                    Ok(log) => {
                        (U256::from(log.data().tokenId), log.data().owner, log.data().amount)
                    }
                    Err(e) => anyhow::bail!("Failed to extract stake created log: {}", e),
                };
            tracing::info!(
                "Staking completed: token_id = {token_id}, owner = {owner}, amount = {amount}"
            );
        } else {
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
                "Staking completed: token_id = {token_id}, owner = {owner}, amount added = {amount_added}, new total = {new_total}"
            );
        }
        Ok(())
    }

    async fn stake(
        &self,
        provider: impl Provider + Clone,
        value: U256,
        add: bool,
    ) -> Result<PendingTransactionBuilder<Ethereum>, anyhow::Error> {
        let staking = IStaking::new(self.vezkc_address, provider);
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
        send_result.context("Sending stake transaction failed")
    }

    async fn stake_with_permit(
        &self,
        provider: impl Provider + Clone,
        token_address: Address,
        value: U256,
        signer: &impl Signer,
        deadline: u64,
        add: bool,
    ) -> Result<PendingTransactionBuilder<Ethereum>, anyhow::Error> {
        let contract = IERC20Permit::new(token_address, provider.clone());
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
        let permit = Permit { owner, spender: self.vezkc_address, value, nonce, deadline };
        tracing::debug!("Permit: {:?}", permit);
        let domain_separator = contract.DOMAIN_SEPARATOR().call().await?;
        let sig = permit.sign(signer, domain_separator).await?.as_bytes();
        let r = B256::from_slice(&sig[..32]);
        let s = B256::from_slice(&sig[32..64]);
        let v: u8 = sig[64];

        let staking = IStaking::new(self.vezkc_address, provider);
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
        send_result.context("Sending stake with permit transaction failed")
    }
}
