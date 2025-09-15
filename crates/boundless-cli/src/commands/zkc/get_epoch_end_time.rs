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
    primitives::{Address, U256},
    providers::{Provider, ProviderBuilder},
};
use anyhow::Context;
use boundless_zkc::{contracts::IZKC, deployments::Deployment};
use chrono::DateTime;
use clap::Args;

use crate::config::GlobalConfig;

/// Command to get the given epoch end time for ZKC.
#[non_exhaustive]
#[derive(Args, Clone, Debug)]
pub struct ZkcGetEpochEndTime {
    /// Configuration for the ZKC deployment to use.
    #[clap(flatten, next_help_heading = "ZKC Deployment")]
    pub deployment: Option<Deployment>,
    /// Epoch to get the end time for.
    pub epoch: u32,
}

impl ZkcGetEpochEndTime {
    /// Run the [ZkcGetEpochEndTime] command.
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

        let end_time = get_epoch_end_time(provider, deployment.zkc_address, self.epoch).await?;
        let datetime = DateTime::from_timestamp(u64::try_from(end_time)? as i64, 0)
            .context("failed to create DateTime")?;
        tracing::info!(
            "Epoch {} end time UTC: {}",
            self.epoch,
            datetime.format("%Y-%m-%d %H:%M:%S")
        );

        Ok(())
    }
}

/// Get current epoch for a specified address.
pub async fn get_epoch_end_time(
    provider: impl Provider,
    zkc_address: Address,
    epoch: u32,
) -> anyhow::Result<U256> {
    let zkc = IZKC::new(zkc_address, provider);
    let result = zkc.getEpochEndTime(U256::from(epoch)).call().await?;
    Ok(result)
}
