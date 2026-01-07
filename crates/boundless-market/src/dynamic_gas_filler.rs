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

use std::sync::Arc;

use alloy::{
    consensus::BlockHeader,
    eips::{eip1559::Eip1559Estimation, BlockNumberOrTag},
    network::{BlockResponse, Network, TransactionBuilder},
    primitives::Address,
    providers::{
        fillers::{FillerControlFlow, GasFillable, GasFiller, TxFiller},
        utils::{self, Eip1559Estimator, Eip1559EstimatorFn},
        Provider, RootProvider, SendableTx,
    },
    transports::{RpcError, TransportResult},
};
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

const DEFAULT_FEE_HISTORY_PERCENTILE: f64 = utils::EIP1559_FEE_ESTIMATION_REWARD_PERCENTILE;
const DEFAULT_FEE_HISTORY_PERCENTILES: [f64; 1] = [DEFAULT_FEE_HISTORY_PERCENTILE];
const LOW_PRIORITY_PERCENTILE: f64 = 20.0;
const MEDIUM_PRIORITY_PERCENTILE: f64 = 30.0;
const HIGH_PRIORITY_PERCENTILE: f64 = 50.0;
const DEFAULT_BASE_FEE_MULTIPLIER_PERCENTAGE: u64 =
    (utils::EIP1559_BASE_FEE_MULTIPLIER as u64) * 100;

/// Priority mode for transaction gas pricing.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum PriorityMode {
    /// Uses the 20th percentile from eth_feeHistory with a dynamic multiplier of `+3%` per pending transaction.
    Low,
    /// Uses the 30th percentile from eth_feeHistory with a dynamic multiplier of `+5%` per pending transaction.
    #[default]
    Medium,
    /// Uses the 50th percentile from eth_feeHistory with a dynamic multiplier of `+7%` per pending transaction.
    High,
    /// Uses a custom percentile from eth_feeHistory with a configurable dynamic multiplier per pending transaction.
    Custom {
        /// Multiplier percentage for the base fee component when building EIP-1559 estimates
        /// (e.g. `200` doubles the base fee similar to Alloy defaults).
        #[serde(default = "default_custom_base_fee_multiplier_percentage")]
        base_fee_multiplier_percentage: u64,
        /// Percentage multiplier applied to the computed priority fee (100 = unchanged, 120 = +20%).
        #[serde(
            default = "default_custom_priority_fee_multiplier_percentage",
            alias = "multiplier_percentage"
        )]
        priority_fee_multiplier_percentage: u64,
        /// The percentile to request via `eth_feeHistory` for priority fee estimation.
        #[serde(default = "default_custom_priority_fee_percentile")]
        priority_fee_percentile: f64,
        /// The incremental percentage applied per pending tx when scaling the final estimate.
        #[serde(default = "default_custom_dynamic_multiplier_percentage")]
        dynamic_multiplier_percentage: u64,
    },
}

const fn default_custom_base_fee_multiplier_percentage() -> u64 {
    DEFAULT_BASE_FEE_MULTIPLIER_PERCENTAGE
}

const fn default_custom_priority_fee_multiplier_percentage() -> u64 {
    100
}

const fn default_custom_priority_fee_percentile() -> f64 {
    DEFAULT_FEE_HISTORY_PERCENTILE
}

const fn default_custom_dynamic_multiplier_percentage() -> u64 {
    5
}

impl PriorityMode {
    /// Returns the configuration for this priority mode.
    fn config(self) -> PriorityModeConfig {
        match self {
            PriorityMode::Low => PriorityModeConfig {
                base_fee_multiplier_percentage: DEFAULT_BASE_FEE_MULTIPLIER_PERCENTAGE,
                priority_fee_multiplier_percentage: 100,
                priority_fee_percentile: LOW_PRIORITY_PERCENTILE,
                dynamic_multiplier_percentage: 3,
            },
            PriorityMode::Medium => PriorityModeConfig {
                base_fee_multiplier_percentage: DEFAULT_BASE_FEE_MULTIPLIER_PERCENTAGE,
                priority_fee_multiplier_percentage: 100,
                priority_fee_percentile: MEDIUM_PRIORITY_PERCENTILE,
                dynamic_multiplier_percentage: 5,
            },
            PriorityMode::High => PriorityModeConfig {
                base_fee_multiplier_percentage: 250,
                priority_fee_multiplier_percentage: 100,
                priority_fee_percentile: HIGH_PRIORITY_PERCENTILE,
                dynamic_multiplier_percentage: 7,
            },
            PriorityMode::Custom {
                base_fee_multiplier_percentage,
                priority_fee_multiplier_percentage,
                priority_fee_percentile,
                dynamic_multiplier_percentage,
            } => PriorityModeConfig {
                base_fee_multiplier_percentage,
                priority_fee_multiplier_percentage,
                priority_fee_percentile,
                dynamic_multiplier_percentage,
            },
        }
    }
}

/// Configuration for a priority mode.
#[derive(Clone, Debug)]
struct PriorityModeConfig {
    /// Multiplier percentage applied to the base fee estimate.
    base_fee_multiplier_percentage: u64,
    /// Multiplier percentage applied to the priority fee estimate.
    priority_fee_multiplier_percentage: u64,
    /// The percentile used when fetching fee history for priority fee estimation.
    priority_fee_percentile: f64,
    /// The incremental percentage applied to the base fee and priority fee per pending transaction (e.g., 0 = no change, 5 = +5% per pending tx).
    dynamic_multiplier_percentage: u64,
}

#[derive(Clone, Copy, Debug)]
struct CustomizedFeeEstimator {
    base_fee_multiplier_percentage: u64,
    priority_fee_multiplier_percentage: u64,
}

impl Eip1559EstimatorFn for CustomizedFeeEstimator {
    fn estimate(&self, base_fee: u128, rewards: &[Vec<u128>]) -> Eip1559Estimation {
        let max_priority_fee_per_gas = estimate_priority_fee(rewards);
        let scaled_priority_fee = max_priority_fee_per_gas
            .saturating_mul(self.priority_fee_multiplier_percentage as u128)
            / 100;
        let potential_max_fee =
            base_fee.saturating_mul(self.base_fee_multiplier_percentage as u128) / 100;

        Eip1559Estimation {
            max_fee_per_gas: potential_max_fee + scaled_priority_fee,
            max_priority_fee_per_gas: scaled_priority_fee,
        }
    }
}

// Note: copied function from alloy as this is a private function.
fn estimate_priority_fee(rewards: &[Vec<u128>]) -> u128 {
    let mut rewards: Vec<u128> = rewards
        .iter()
        .filter_map(|reward| reward.first().copied())
        .filter(|reward| *reward > 0_u128)
        .collect();

    if rewards.is_empty() {
        return utils::EIP1559_MIN_PRIORITY_FEE;
    }

    rewards.sort_unstable();
    let mid = rewards.len() / 2;
    let median =
        if rewards.len() % 2 == 0 { (rewards[mid - 1] + rewards[mid]) / 2 } else { rewards[mid] };

    std::cmp::max(median, utils::EIP1559_MIN_PRIORITY_FEE)
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
        self.priority_mode.read().await.clone()
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
        let priority_config = self.get_priority_mode().await.config();

        let fee_override_provider = FeeEstimatorProvider::new(
            provider,
            priority_config.priority_fee_percentile,
            priority_config.base_fee_multiplier_percentage,
            priority_config.priority_fee_multiplier_percentage,
        );

        let fillable = GasFiller.prepare(&fee_override_provider, tx).await?;

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

        let multiplier = tx_diff.saturating_mul(priority_config.dynamic_multiplier_percentage);

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

struct FeeEstimatorProvider<'a, P, N> {
    inner: &'a P,
    priority_fee_percentile: f64,
    base_fee_multiplier_percentage: u64,
    priority_fee_multiplier_percentage: u64,
    _network: std::marker::PhantomData<N>,
}

impl<'a, P, N> FeeEstimatorProvider<'a, P, N> {
    fn new(
        inner: &'a P,
        priority_fee_percentile: f64,
        base_fee_multiplier_percentage: u64,
        priority_fee_multiplier_percentage: u64,
    ) -> Self {
        Self {
            inner,
            priority_fee_percentile,
            base_fee_multiplier_percentage,
            priority_fee_multiplier_percentage,
            _network: std::marker::PhantomData,
        }
    }

    fn reward_percentiles(&self) -> &[f64] {
        if self.priority_fee_percentile == 0.0 {
            &DEFAULT_FEE_HISTORY_PERCENTILES
        } else {
            std::slice::from_ref(&self.priority_fee_percentile)
        }
    }
}

#[async_trait::async_trait]
impl<'a, P, N> Provider<N> for FeeEstimatorProvider<'a, P, N>
where
    P: Provider<N>,
    N: Network,
{
    fn root(&self) -> &RootProvider<N> {
        self.inner.root()
    }

    async fn estimate_eip1559_fees_with(
        &self,
        estimator: Eip1559Estimator,
    ) -> TransportResult<Eip1559Estimation> {
        let fee_history = self
            .inner
            .get_fee_history(
                utils::EIP1559_FEE_ESTIMATION_PAST_BLOCKS,
                BlockNumberOrTag::Latest,
                self.reward_percentiles(),
            )
            .await?;

        let base_fee_per_gas = match fee_history.latest_block_base_fee() {
            Some(base_fee) if base_fee != 0 => base_fee,
            _ => self
                .inner
                .get_block_by_number(BlockNumberOrTag::Latest)
                .await?
                .ok_or(RpcError::NullResp)?
                .header()
                .as_ref()
                .base_fee_per_gas()
                .ok_or(RpcError::UnsupportedFeature("eip1559"))?
                .into(),
        };

        Ok(estimator.estimate(base_fee_per_gas, &fee_history.reward.unwrap_or_default()))
    }

    async fn estimate_eip1559_fees(&self) -> TransportResult<Eip1559Estimation> {
        let estimator = Eip1559Estimator::new_estimator(CustomizedFeeEstimator {
            base_fee_multiplier_percentage: self.base_fee_multiplier_percentage,
            priority_fee_multiplier_percentage: self.priority_fee_multiplier_percentage,
        });

        self.estimate_eip1559_fees_with(estimator).await
    }
}
