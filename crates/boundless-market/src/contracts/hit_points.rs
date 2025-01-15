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

use std::time::Duration;

use crate::contracts::{
    token::IERC20,
    IHitPoints::{self, IHitPointsErrors},
};

use super::{IHitPoints::IHitPointsInstance, TXN_CONFIRM_TIMEOUT};
use alloy::{network::Ethereum, primitives::Address, providers::Provider, transports::Transport};
use alloy_primitives::U256;
use anyhow::{Context, Result};

/// HitPointsService provides a high-level interface to the HitPoints contract.
#[derive(Clone)]
pub struct HitPointsService<T, P> {
    instance: IHitPointsInstance<T, P, Ethereum>,
    caller: Address,
    tx_timeout: Duration,
}

impl<T, P> HitPointsService<T, P>
where
    T: Transport + Clone,
    P: Provider<T, Ethereum> + 'static + Clone,
{
    /// Creates a new HitPointsService.
    pub fn new(address: Address, provider: P, caller: Address) -> Self {
        let instance = IHitPoints::new(address, provider);

        Self { instance, caller, tx_timeout: TXN_CONFIRM_TIMEOUT }
    }

    /// Returns the underlying IHitPointsInstance.
    pub fn instance(&self) -> &IHitPointsInstance<T, P, Ethereum> {
        &self.instance
    }

    /// Sets the timeout for transaction confirmation.
    pub fn with_timeout(self, tx_timeout: Duration) -> Self {
        Self { tx_timeout, ..self }
    }

    /// Add an account to the authorized list.
    pub async fn authorize(&self, account: Address) -> Result<()> {
        tracing::debug!("Calling authorize({:?})", account);
        let call = self.instance.authorize(account).from(self.caller);
        let pending_tx = call.send().await.map_err(IHitPointsErrors::decode_error)?;
        tracing::debug!("Broadcasting tx {}", pending_tx.tx_hash());
        let tx_hash = pending_tx
            .with_timeout(Some(self.tx_timeout))
            .watch()
            .await
            .context("failed to confirm tx")?;

        tracing::info!("Authorized {}: {}", account, tx_hash);

        Ok(())
    }

    /// Mint HitPoints for an account.
    pub async fn mint(&self, account: Address, value: U256) -> Result<()> {
        tracing::debug!("Calling mint({:?}, {})", account, value);
        let call = self.instance.mint(account, value).from(self.caller);
        let pending_tx = call.send().await.map_err(IHitPointsErrors::decode_error)?;
        tracing::debug!("Broadcasting tx {}", pending_tx.tx_hash());
        let tx_hash = pending_tx
            .with_timeout(Some(self.tx_timeout))
            .watch()
            .await
            .context("failed to confirm tx")?;

        tracing::info!("Minted {} for {}: {}", value, account, tx_hash);

        Ok(())
    }

    /// Returns the balance of an account.
    pub async fn balance_of(&self, account: Address) -> Result<U256> {
        tracing::debug!("Calling balanceOf({:?})", account);
        let contract = IERC20::new(*self.instance.address(), self.instance.provider());
        let call = contract.balanceOf(account).from(self.caller);
        let balance = call.call().await.map_err(IHitPointsErrors::decode_error)?;
        return Ok(balance._0);
    }
}
