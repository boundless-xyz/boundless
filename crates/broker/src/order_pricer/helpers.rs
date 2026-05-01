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

//! Pricing logic and the [`OrderPricingContext`] trait implementation for
//! [`OrderPricer`]. The trait methods provide the hooks that the upstream
//! `boundless_market::prover_utils::price_order` function calls into.

use std::collections::HashSet;
use std::sync::Arc;

use crate::OrderPricingOutcome::{Lock, ProveAfterLockExpire, Skip};
use crate::{
    now_timestamp, utils, FulfillmentType, OrderPricingContext, OrderPricingError,
    OrderPricingOutcome, OrderRequest,
};
use alloy::{
    network::Ethereum,
    primitives::{utils::format_ether, Address, U256},
    providers::{Provider, WalletProvider},
};
use anyhow::Context;
use boundless_market::price_oracle::{Amount, Asset};
use boundless_market::prover_utils::apply_secondary_fulfillment_discount;
use boundless_market::telemetry::EvalOutcome;
use boundless_market::{
    prover_utils::estimate_erc1271_gas, selector::SupportedSelectors, storage::StorageDownloader,
};
use tokio_util::sync::CancellationToken;

use super::error::{
    OrderPricerErr, SKIP_ALREADY_FULFILLED, SKIP_ALREADY_LOCKED, SKIP_FETCH_ERROR,
    SKIP_GAS_EXCEEDS_BALANCE, SKIP_GAS_EXCEEDS_MAX_PRICE, SKIP_INSUFFICIENT_COLLATERAL,
    SKIP_IN_DENYLIST, SKIP_NOT_IN_ALLOWLIST, SKIP_PRICING_ERROR, SKIP_UNSUPPORTED_SELECTOR_BROKER,
};
use super::service::OrderPricer;
use crate::config::MarketConfig;
use crate::provers::ProverObj;
use crate::PreflightCache;

impl<P> OrderPricer<P>
where
    P: Provider<Ethereum> + 'static + Clone + WalletProvider,
{
    pub(super) async fn price_order_and_update_state(
        &self,
        mut order: Box<OrderRequest>,
        cancel_token: CancellationToken,
    ) -> bool {
        let order_id = order.id();
        let queue_duration_ms = order
            .created_at
            .map(|t| t.elapsed().as_millis() as u64)
            .unwrap_or_else(|| now_timestamp().saturating_sub(order.received_at_timestamp) * 1000);

        let f = || async {
            let pricing_result = tokio::select! {
                result = self.price_order(&mut order) => result,
                _ = cancel_token.cancelled() => {
                    tracing::info!("Order pricing cancelled during pricing for order {order_id}");
                    return Ok(false);
                }
            };

            let total_elapsed_ms =
                order.created_at.map(|t| t.elapsed().as_millis() as u64).unwrap_or_else(|| {
                    now_timestamp().saturating_sub(order.received_at_timestamp) * 1000
                });
            let preflight_duration_ms = total_elapsed_ms.saturating_sub(queue_duration_ms);

            let (accepted, skip_code, skip_reason, total_cycles, eval_outcome) =
                match pricing_result {
                    Ok(Lock {
                        total_cycles,
                        target_timestamp_secs,
                        expiry_secs,
                        target_mcycle_price,
                        max_mcycle_price,
                        current_mcycle_price,
                        config_min_mcycle_price,
                        journal_len,
                    }) => {
                        order.total_cycles = Some(total_cycles);
                        order.journal_bytes = Some(journal_len);
                        order.target_timestamp = Some(target_timestamp_secs);
                        order.expire_timestamp = Some(expiry_secs);

                        tracing::info!(
                            "Order {order_id} scheduled for lock attempt in {}s (timestamp: {}), when price exceeds: {} ETH/Mcycle (config min price: {} ETH/Mcycle, current price: {} ETH/Mcycle, max price: {} ETH/Mcycle)",
                            target_timestamp_secs.saturating_sub(now_timestamp()),
                            target_timestamp_secs,
                            format_ether(target_mcycle_price),
                            format_ether(config_min_mcycle_price),
                            format_ether(current_mcycle_price),
                            format_ether(max_mcycle_price),
                        );

                        (true, None, None, Some(total_cycles), EvalOutcome::Locked)
                    }
                    Ok(ProveAfterLockExpire {
                        total_cycles,
                        lock_expire_timestamp_secs,
                        expiry_secs,
                        mcycle_price,
                        config_min_mcycle_price,
                        journal_len,
                    }) => {
                        order.journal_bytes = Some(journal_len);
                        tracing::info!("Setting order {order_id} to prove after lock expiry at {lock_expire_timestamp_secs} (projected price: {} ZKC/Mcycle, config min price: {} ZKC/Mcycle)", self.format_collateral(mcycle_price), self.format_collateral(config_min_mcycle_price));
                        order.total_cycles = Some(total_cycles);
                        order.target_timestamp = Some(lock_expire_timestamp_secs);
                        order.expire_timestamp = Some(expiry_secs);

                        // Compute discounted collateral reward, convert ZKC -> USD -> ETH.
                        // Later used in order prioritization. This value does not change based on time,
                        // the only variable is the price of ZKC.
                        let raw_collateral =
                            order.request.offer.collateral_reward_if_locked_and_not_fulfilled();
                        let config = self.market_config()?;
                        let discounted = apply_secondary_fulfillment_discount(
                            raw_collateral,
                            config.expected_probability_win_secondary_fulfillment,
                        );
                        let zkc_amount = Amount::new(discounted, Asset::ZKC);
                        let eth_amount = self
                            .price_oracle
                            .convert(&zkc_amount, Asset::ETH)
                            .await
                            .map_err(anyhow::Error::from)?;
                        order.expected_reward_eth = Some(eth_amount.value);
                        tracing::debug!(
                            "Secondary order {order_id}: raw_collateral={} ZKC, win_probability={}%, discounted={} ZKC, expected_reward={} ETH",
                            self.format_collateral(raw_collateral),
                            config.expected_probability_win_secondary_fulfillment,
                            self.format_collateral(discounted),
                            format_ether(eth_amount.value),
                        );

                        order.priced_at_timestamp = Some(now_timestamp());
                        (true, None, None, Some(total_cycles), EvalOutcome::FulfillAfterLockExpire)
                    }
                    Ok(Skip { code, reason }) => {
                        tracing::info!("Skipping order {order_id}: {reason}");
                        (false, Some(code), Some(reason), order.total_cycles, EvalOutcome::Skipped)
                    }
                    Err(
                        OrderPricingError::FetchInputErr(ref inner)
                        | OrderPricingError::FetchImageErr(ref inner),
                    ) => {
                        let reason = format!("failed to fetch input/image: {inner:#}");
                        tracing::info!(
                            "Skipping order {order_id}: {reason} (this is expected for private inputs that require specific storage credentials)"
                        );
                        (false, Some(SKIP_FETCH_ERROR), Some(reason), None, EvalOutcome::Skipped)
                    }
                    Err(err) => {
                        tracing::warn!("Failed to price order {order_id}: {err}");
                        (
                            false,
                            Some(SKIP_PRICING_ERROR),
                            Some(err.to_string()),
                            None,
                            EvalOutcome::Skipped,
                        )
                    }
                };

            crate::telemetry::telemetry(self.chain_id).record_order_pricing(
                &order,
                eval_outcome,
                skip_code,
                skip_reason,
                total_cycles,
                queue_duration_ms,
                preflight_duration_ms,
            );

            if accepted {
                self.priced_orders_tx
                    .send(order)
                    .await
                    .context("Failed to send to order_result_tx")?;
                Ok::<_, OrderPricerErr>(true)
            } else {
                Ok(false)
            }
        };

        match f().await {
            Ok(true) => true,
            Ok(false) => false,
            Err(err) => {
                tracing::error!("Failed to update for order {order_id}: {err}");
                false
            }
        }
    }

    async fn gas_balance_reserved(&self) -> Result<U256, OrderPricerErr> {
        let gas_price =
            self.chain_monitor.current_gas_price().await.context("Failed to get gas price")?;
        let fulfill_pending_gas = self.estimate_gas_to_fulfill_pending().await?;
        Ok(U256::from(gas_price) * U256::from(fulfill_pending_gas))
    }
}

impl<P> OrderPricingContext for OrderPricer<P>
where
    P: Provider<Ethereum> + 'static + Clone + WalletProvider,
{
    fn market_config(&self) -> Result<MarketConfig, OrderPricerErr> {
        let config = self.config.lock_all().context("Failed to read config")?;
        Ok(config.market.clone())
    }

    fn denied_requestor_addresses(&self) -> Result<Option<HashSet<Address>>, OrderPricerErr> {
        let config = self.config.lock_all().context("Failed to read config")?;
        Ok(config.market.deny_requestor_addresses.clone())
    }

    fn supported_selectors(&self) -> &SupportedSelectors {
        &self.supported_selectors
    }

    fn preflight_cache(&self) -> &PreflightCache {
        &self.preflight_cache
    }

    fn collateral_token_decimals(&self) -> u8 {
        self.collateral_token_decimals
    }

    fn check_requestor_allowed(
        &self,
        order: &OrderRequest,
        denied_addresses_opt: Option<&HashSet<Address>>,
    ) -> Result<Option<OrderPricingOutcome>, OrderPricerErr> {
        let client_addr = order.request.client_address();
        if !self.allow_requestors.is_allow_requestor(&client_addr) {
            let has_allow_list = {
                let config = self.config.lock_all().context("Failed to read config")?;
                config.market.allow_client_addresses.is_some()
                    || config.market.allow_requestor_lists.is_some()
            };
            if has_allow_list {
                return Ok(Some(Skip {
                    code: SKIP_NOT_IN_ALLOWLIST,
                    reason: format!("order from {client_addr} is not in allow requestors"),
                }));
            }
        }

        if let Some(deny_addresses) = denied_addresses_opt {
            if deny_addresses.contains(&client_addr) {
                return Ok(Some(Skip {
                    code: SKIP_IN_DENYLIST,
                    reason: format!(
                        "order is from {} and is in deny_requestor_addresses",
                        client_addr
                    ),
                }));
            }
        }

        if !self.supported_selectors.is_supported(order.request.requirements.selector) {
            return Ok(Some(Skip {
                code: SKIP_UNSUPPORTED_SELECTOR_BROKER,
                reason: format!(
                    "unsupported selector requirement. Requested: {:x}. Supported: {:?}",
                    order.request.requirements.selector,
                    self.supported_selectors
                        .selectors
                        .iter()
                        .map(|(k, v)| format!("{k:x} ({v:?})"))
                        .collect::<Vec<_>>()
                ),
            }));
        };

        Ok(None)
    }

    async fn check_request_available(
        &self,
        order: &OrderRequest,
    ) -> Result<Option<OrderPricingOutcome>, OrderPricerErr> {
        let request_id = U256::from(order.request.id);
        let is_locked = self
            .db
            .is_request_locked(request_id)
            .await
            .context("Failed to check if request is locked before pricing")?;
        let is_fulfilled = self
            .db
            .is_request_fulfilled(request_id)
            .await
            .context("Failed to check if request is fulfilled before pricing")?;

        if order.fulfillment_type == FulfillmentType::LockAndFulfill && is_locked {
            return Ok(Some(Skip {
                code: SKIP_ALREADY_LOCKED,
                reason: "order is already locked".to_string(),
            }));
        }

        if order.fulfillment_type == FulfillmentType::FulfillAfterLockExpire && is_fulfilled {
            return Ok(Some(Skip {
                code: SKIP_ALREADY_FULFILLED,
                reason: "order is already fulfilled".to_string(),
            }));
        }

        Ok(None)
    }

    async fn estimate_gas_to_fulfill_pending(&self) -> Result<u64, OrderPricerErr> {
        let mut gas = 0;
        let config = &self.config;
        for order in self
            .db
            .get_committed_orders()
            .await
            .map_err(|err| OrderPricerErr::UnexpectedErr(Arc::new(err.into())))?
        {
            let gas_estimate = utils::estimate_gas_to_fulfill(
                config,
                &self.supported_selectors,
                &order.request,
                order.journal_bytes,
            )
            .await?;
            gas += gas_estimate;
        }
        tracing::debug!("Total gas estimate to fulfill pending orders: {}", gas);
        Ok(gas)
    }

    fn is_priority_requestor(&self, client_addr: &Address) -> bool {
        self.priority_requestors.is_priority_requestor(client_addr)
    }

    async fn check_available_balances(
        &self,
        order: &OrderRequest,
        order_gas_cost: U256,
        lock_expired: bool,
        lockin_collateral: U256,
    ) -> Result<Option<OrderPricingOutcome>, OrderPricerErr> {
        if self.listen_only {
            // In listen-only mode, skip all balance checks since no transactions will be sent.
            return Ok(None);
        }

        let balance = self
            .provider
            .get_balance(self.provider.default_signer_address())
            .await
            .map_err(|err| OrderPricerErr::RpcErr(Arc::new(err.into())))?;

        let gas_balance_reserved = self.gas_balance_reserved().await?;

        let available_gas = balance.saturating_sub(gas_balance_reserved);
        tracing::debug!(
            "available gas balance: (account_balance) {} - (expected_future_gas) {} = {}",
            format_ether(balance),
            format_ether(gas_balance_reserved),
            format_ether(available_gas)
        );

        let available_collateral = self
            .market
            .balance_of_collateral(self.provider.default_signer_address())
            .await
            .map_err(|err| OrderPricerErr::RpcErr(Arc::new(err.into())))?;

        let skip_gas_check =
            self.config.lock_all().map(|c| c.market.skip_gas_profitability_check).unwrap_or(false);

        // TODO: Once we have a price feed for the collateral token in gas tokens, extend
        // this check to cover lock_expired orders (where the reward is a fraction of the collateral).
        if !skip_gas_check && order_gas_cost > order.request.offer.maxPrice && !lock_expired {
            return Ok(Some(Skip {
                code: SKIP_GAS_EXCEEDS_MAX_PRICE,
                reason: format!(
                    "estimated gas cost to lock and fulfill order of {} exceeds max price of {}",
                    format_ether(order_gas_cost),
                    format_ether(order.request.offer.maxPrice)
                ),
            }));
        }

        if order_gas_cost > available_gas {
            return Ok(Some(Skip {
                code: SKIP_GAS_EXCEEDS_BALANCE,
                reason: format!(
                    "estimated gas cost to lock and fulfill order of {} exceeds available gas of {}",
                    format_ether(order_gas_cost),
                    format_ether(available_gas)
                ),
            }));
        }

        if !lock_expired && lockin_collateral > available_collateral {
            return Ok(Some(Skip {
                code: SKIP_INSUFFICIENT_COLLATERAL,
                reason: format!(
                    "insufficient available collateral to lock order. Requires {} but has {}",
                    self.format_collateral(lockin_collateral),
                    self.format_collateral(available_collateral),
                ),
            }));
        }

        Ok(None)
    }

    async fn current_gas_price(&self) -> Result<u128, OrderPricerErr> {
        Ok(self.chain_monitor.current_gas_price().await.context("Failed to get gas price")?)
    }

    async fn estimate_erc1271_gas(&self, order: &OrderRequest) -> u64 {
        estimate_erc1271_gas(order, &self.provider, &self.erc1271_gas_cache).await
    }

    async fn convert_to_eth(&self, amount: &Amount) -> Result<Amount, OrderPricerErr> {
        if amount.asset == Asset::ETH {
            return Ok(amount.clone());
        }

        self.price_oracle.convert(amount, Asset::ETH).await.map_err(|e| {
            OrderPricerErr::UnexpectedErr(Arc::new(anyhow::anyhow!(
                "Failed to convert {} to ETH: {}",
                amount,
                e
            )))
        })
    }

    async fn convert_to_zkc(&self, amount: &Amount) -> Result<Amount, OrderPricerErr> {
        if amount.asset == Asset::ZKC {
            return Ok(amount.clone());
        }

        self.price_oracle.convert(amount, Asset::ZKC).await.map_err(|e| {
            OrderPricerErr::UnexpectedErr(Arc::new(anyhow::anyhow!(
                "Failed to convert {} to ZKC: {}",
                amount,
                e
            )))
        })
    }

    fn prover(&self) -> &ProverObj {
        &self.prover
    }

    fn downloader(&self) -> Arc<dyn StorageDownloader + Send + Sync> {
        Arc::new(self.downloader.clone())
    }
}
