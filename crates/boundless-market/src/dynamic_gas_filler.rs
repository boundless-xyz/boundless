// Copyright 2025 Boundless Foundation, Inc.
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

use std::sync::Arc;

use alloy::{
    network::{Network, TransactionBuilder},
    primitives::Address,
    providers::{
        fillers::{FillerControlFlow, GasFillable, GasFiller, TxFiller},
        Provider, SendableTx,
    },
    transports::TransportResult,
};
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

/// Priority mode for transaction gas pricing.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum PriorityMode {
    /// Use the eth_feeHistory as is and only add `+3%` per pending tx.
    Low,
    /// Add a `+20%` increase to the base/priority fee and `+5%` per pending transaction.
    #[default]
    Medium,
    /// Add a `+30%` increase to the base/priority fee and `+7%` per pending transaction.
    High,
    /// Add a custom static increase to the base/priority fee and `+5%` per pending transaction.
    Custom {
        /// The static multiplier percentage to apply (0 = no change).
        multiplier_percentage: u64,
    },
}

impl PriorityMode {
    /// Returns the configuration for this priority mode.
    fn config(self) -> PriorityModeConfig {
        match self {
            PriorityMode::Low => PriorityModeConfig {
                estimate_additional_percentage: 0,
                dynamic_multiplier_percentage: 3,
            },
            PriorityMode::Medium => PriorityModeConfig {
                estimate_additional_percentage: 20,
                dynamic_multiplier_percentage: 5,
            },
            PriorityMode::High => PriorityModeConfig {
                estimate_additional_percentage: 30,
                dynamic_multiplier_percentage: 7,
            },
            PriorityMode::Custom { multiplier_percentage } => PriorityModeConfig {
                estimate_additional_percentage: multiplier_percentage,
                dynamic_multiplier_percentage: 5,
            },
        }
    }
}

/// Configuration for a priority mode.
#[derive(Clone, Debug)]
struct PriorityModeConfig {
    /// The base percentage increase applied to the base fee and priority fee of every transaction (e.g., 0 = no change, 10 = 10% increase).
    estimate_additional_percentage: u64,
    /// The incremental percentage applied to the base fee and priority fee per pending transaction (e.g., 0 = no change, 5 = +5% per pending tx).
    dynamic_multiplier_percentage: u64,
}

/// A gas filler that adjusts gas prices based on priority mode.
#[derive(Clone)]
pub struct DynamicGasFiller {
    /// The percentage to set the gas limit to, relative to estimate (e.g., 120 = 20% increase).
    pub additional_gas_limit_percentage: u64,
    /// The current priority mode (adjustable via RwLock).
    pub priority_mode: Arc<RwLock<PriorityMode>>,
    /// The address to check the pending transaction count for.
    pub address: Address,
}

impl std::fmt::Debug for DynamicGasFiller {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DynamicGasFiller")
            .field("gas_limit_percentage", &self.additional_gas_limit_percentage)
            .field("address", &self.address)
            .finish()
    }
}

impl DynamicGasFiller {
    /// Creates a new `DynamicGasFiller`.
    ///
    /// # Arguments
    ///
    /// * `additional_gas_limit_percentage` - The percentage to set the gas limit to, relative to estimate (e.g., 20 = 20% increase).
    /// * `priority_mode` - The initial priority mode.
    /// * `address` - The address to check the pending transaction count for.
    pub fn new(
        additional_gas_limit_percentage: u64,
        priority_mode: PriorityMode,
        address: Address,
    ) -> Self {
        Self {
            additional_gas_limit_percentage,
            priority_mode: Arc::new(RwLock::new(priority_mode)),
            address,
        }
    }

    /// Sets the priority mode.
    pub async fn set_priority_mode(&self, mode: PriorityMode) {
        *self.priority_mode.write().await = mode;
    }

    /// Gets the current priority mode.
    pub async fn get_priority_mode(&self) -> PriorityMode {
        *self.priority_mode.read().await
    }
}

/// Parameters for the dynamic gas filler.
pub struct DynamicGasParams {
    /// The fillable gas parameters.
    pub fillable: GasFillable,
    /// The multiplier to apply to the gas limit.
    pub multiplier: u64,
}

impl<N: Network> TxFiller<N> for DynamicGasFiller {
    type Fillable = DynamicGasParams;

    fn status(&self, tx: &<N as Network>::TransactionRequest) -> FillerControlFlow {
        TxFiller::<N>::status(&GasFiller, tx)
    }

    fn fill_sync(&self, _tx: &mut SendableTx<N>) {}

    async fn prepare<P>(
        &self,
        provider: &P,
        tx: &N::TransactionRequest,
    ) -> TransportResult<Self::Fillable>
    where
        P: Provider<N>,
    {
        const _: () = {
            // Assumptions about the default GasFiller params.
            // Using default, as it is requires a lot of duplication to override.
            assert!(alloy::providers::utils::EIP1559_FEE_ESTIMATION_REWARD_PERCENTILE == 20.0);
            assert!(alloy::providers::utils::EIP1559_BASE_FEE_MULTIPLIER == 2);
        };
        let fillable = GasFiller.prepare(provider, tx).await?;

        // Calculate dynamic multiplier based on pending transactions
        let confirmed_nonce = provider.get_transaction_count(self.address).latest().await?;
        let pending_nonce = provider.get_transaction_count(self.address).pending().await?;

        let tx_diff = pending_nonce.saturating_sub(confirmed_nonce);
        tracing::debug!(
            "DynamicGasFiller: Pending transactions: {}, confirmed transactions: {} - tx_diff: {}",
            pending_nonce,
            confirmed_nonce,
            tx_diff
        );

        let priority_config = self.get_priority_mode().await.config();

        let multiplier = priority_config.estimate_additional_percentage
            + tx_diff.saturating_mul(priority_config.dynamic_multiplier_percentage);

        Ok(DynamicGasParams { fillable, multiplier })
    }

    async fn fill(
        &self,
        params: Self::Fillable,
        mut tx: SendableTx<N>,
    ) -> TransportResult<SendableTx<N>> {
        if let Some(builder) = tx.as_mut_builder() {
            match params.fillable {
                GasFillable::Legacy { gas_limit, mut gas_price } => {
                    gas_price = gas_price.saturating_mul(100 + params.multiplier as u128) / 100;
                    let adjusted_gas_limit =
                        gas_limit.saturating_mul(100 + self.additional_gas_limit_percentage) / 100;
                    builder.set_gas_limit(adjusted_gas_limit);
                    builder.set_gas_price(gas_price);
                }
                GasFillable::Eip1559 { gas_limit, mut estimate } => {
                    estimate.scale_by_pct(params.multiplier);
                    let adjusted_gas_limit =
                        gas_limit.saturating_mul(100 + self.additional_gas_limit_percentage) / 100;

                    builder.set_gas_limit(adjusted_gas_limit);
                    builder.set_max_fee_per_gas(estimate.max_fee_per_gas);
                    builder.set_max_priority_fee_per_gas(estimate.max_priority_fee_per_gas);
                    tracing::debug!(
                        "DynamicGasFiller: Adjusted gas limit: {}, max fee: {}, priority fee: {}, dynamic multiplier: {}%",
                        adjusted_gas_limit,
                        estimate.max_fee_per_gas,
                        estimate.max_priority_fee_per_gas,
                        params.multiplier
                    );
                }
            }
        }

        Ok(tx)
    }
}
