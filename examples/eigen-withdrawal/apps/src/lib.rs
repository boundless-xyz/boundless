// Copyright (c) 2024 RISC Zero, Inc.
//
// All rights reserved.

pub mod contracts;

use std::time::Duration;

use crate::{contracts::IEigenPod, IEigenPod::IEigenPodInstance};
use alloy::{
    network::Ethereum,
    primitives::{Address, Bytes, U256},
    providers::Provider,
    sol_types::{SolCall, SolEvent},
    transports::Transport,
};
use anyhow::{bail, Context, Ok, Result};

#[derive(Clone)]
pub struct EigenPodService<T, P> {
    instance: IEigenPodInstance<T, P, Ethereum>,
    caller: Address,
}

impl<T, P> EigenPodService<T, P>
where
    T: Transport + Clone,
    P: Provider<T, Ethereum> + 'static + Clone,
{
    pub const TX_TIMEOUT: Duration = Duration::from_secs(30);

    pub fn new(address: Address, provider: P, caller: Address) -> Self {
        let instance = IEigenPod::new(address, provider);

        EigenPodService { instance, caller }
    }

    pub fn instance(&self) -> &IEigenPodInstance<T, P, Ethereum> {
        &self.instance
    }

    pub fn caller(&self) -> Address {
        self.caller
    }

    pub async fn new_validator(&self, stake: U256) -> Result<(u64, u64)> {
        tracing::debug!("Calling new_validator({})", stake);
        let value = stake;
        let call = self.instance.newValidator().from(self.caller).value(value);
        let pending_tx = call.send().await.context("failed to broadcast tx")?;
        tracing::debug!("Broadcasting tx {}", pending_tx.tx_hash());

        let receipt = pending_tx
            .with_timeout(Some(Self::TX_TIMEOUT))
            .get_receipt()
            .await
            .context("failed to confirm tx")?;
        let [log] = receipt.inner.logs() else { bail!("call must emit exactly one event") };
        let log = log
            .log_decode::<IEigenPod::NewValidator>()
            .with_context(|| format!("call did not emit {}", IEigenPod::NewValidator::SIGNATURE))?;

        Ok((log.inner.data.validatorIndex, log.inner.data.stake))
    }

    pub async fn update_stake(&self, validator_index: u64, amount: U256) -> Result<(u64, u64)> {
        tracing::debug!("Calling update_stake({:?})", amount);
        let call = self.instance.updateStake(validator_index).from(self.caller).value(amount);
        let pending_tx = call.send().await.context("failed to broadcast tx")?;
        tracing::debug!("Broadcasting tx {}", pending_tx.tx_hash());

        let receipt = pending_tx
            .with_timeout(Some(Self::TX_TIMEOUT))
            .get_receipt()
            .await
            .context("failed to confirm tx")?;
        let [log] = receipt.inner.logs() else { bail!("call must emit exactly one event") };
        let log = log
            .log_decode::<IEigenPod::StakeUpdated>()
            .with_context(|| format!("call did not emit {}", IEigenPod::StakeUpdated::SIGNATURE))?;

        Ok((log.inner.data.validatorIndex, log.inner.data.stake))
    }

    pub async fn view_stake(&self, validator_index: u64) -> Result<u64> {
        tracing::debug!("Calling view_stake({validator_index})");
        let stake = self
            .instance
            .stakeBalanceGwei(validator_index)
            .call()
            .await
            .with_context(|| {
                format!("failed to call {}", IEigenPod::stakeBalanceGweiCall::SIGNATURE)
            })?
            ._0;

        Ok(stake)
    }

    pub async fn full_withdrawal(&self, journal: Bytes, seal: Bytes) -> Result<(u64, U256)> {
        tracing::debug!("Calling fullWithdrawal()");
        let call = self.instance.fullWithdrawal(journal, seal).from(self.caller);
        let pending_tx = call.send().await.context("failed to broadcast tx")?;
        tracing::debug!("Broadcasting tx {}", pending_tx.tx_hash());

        let receipt = pending_tx
            .with_timeout(Some(Self::TX_TIMEOUT))
            .get_receipt()
            .await
            .context("failed to confirm tx")?;
        let [log] = receipt.inner.logs() else { bail!("call must emit exactly one event") };
        let log = log.log_decode::<IEigenPod::ValidatorExited>().with_context(|| {
            format!("call did not emit {}", IEigenPod::ValidatorExited::SIGNATURE)
        })?;

        Ok((log.inner.data.validatorIndex, log.inner.data.amount))
    }

    pub async fn partial_withdrawal(&self, journal: Bytes, seal: Bytes) -> Result<u64> {
        tracing::debug!("Calling partial_withdrawal()");
        let call = self.instance.partialWithdrawal(journal, seal).from(self.caller);
        let pending_tx = call.send().await.context("failed to broadcast tx")?;
        tracing::debug!("Broadcasting tx {}", pending_tx.tx_hash());

        let receipt = pending_tx
            .with_timeout(Some(Self::TX_TIMEOUT))
            .get_receipt()
            .await
            .context("failed to confirm tx")?;
        let [log] = receipt.inner.logs() else { bail!("call must emit exactly one event") };
        let log = log
            .log_decode::<IEigenPod::Redeemed>()
            .with_context(|| format!("call did not emit {}", IEigenPod::Redeemed::SIGNATURE))?;

        Ok(log.inner.data.amount)
    }

    pub async fn view_yield(&self, validator_index: u64) -> Result<u64> {
        tracing::debug!("Calling estimate_yield()");
        let yield_ = self
            .instance
            .estimateYield(validator_index)
            .call()
            .await
            .with_context(|| format!("failed to call {}", IEigenPod::estimateYieldCall::SIGNATURE))?
            ._0;

        Ok(yield_)
    }
}
