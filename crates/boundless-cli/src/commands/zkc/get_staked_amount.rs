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
    primitives::{utils::format_ether, Address, U256},
    providers::{Provider, ProviderBuilder},
};
use anyhow::Context;
use boundless_zkc::{
    contracts::{DecodeRevert, IStaking},
    deployments::Deployment,
};
use chrono::DateTime;
use clap::Args;

use crate::config::GlobalConfig;

/// Command to get staked amount and withdrawing time for ZKC.
#[non_exhaustive]
#[derive(Args, Clone, Debug)]
pub struct ZkcGetStakedAmount {
    /// Address to get staked amount for.
    pub account: Address,
    /// Configuration for the ZKC deployment to use.
    #[clap(flatten, next_help_heading = "ZKC Deployment")]
    pub deployment: Option<Deployment>,
}

impl ZkcGetStakedAmount {
    /// Run the [ZkcGetStakedAmount] command.
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

        let (amount, withdrawable_at) =
            get_staked_amount(provider, deployment.vezkc_address, self.account).await?;

        let withdrawable_at: u64 = withdrawable_at.try_into()?;
        tracing::info!("Staked amount: {} ZKC", format_ether(amount));
        if withdrawable_at == 0 {
            tracing::info!("Not withdrawable");
        } else {
            let datetime = DateTime::from_timestamp(withdrawable_at as i64, 0)
                .context("failed to create DateTime")?;
            tracing::info!("Withdrawable from UTC: {}", datetime.format("%Y-%m-%d %H:%M:%S"));
        }

        Ok(())
    }
}

/// Get staked amount and withdrawable time for a specified address.
pub async fn get_staked_amount(
    provider: impl Provider,
    staking_address: Address,
    account: Address,
) -> anyhow::Result<(U256, U256)> {
    let staking = IStaking::new(staking_address, provider);
    let result = staking
        .getStakedAmountAndWithdrawalTime(account)
        .call()
        .await
        .maybe_decode_revert::<IStaking::IStakingErrors>()?;
    Ok((result.amount, result.withdrawableAt))
}
