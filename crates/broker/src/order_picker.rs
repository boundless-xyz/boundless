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

use hex::FromHex;
use risc0_zkvm::sha::Digest;
use sha2::{Digest as Sha2Digest, Sha256};
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

#[cfg(test)]
use crate::config::MarketConf;
use crate::{
    chain_monitor::ChainMonitorService,
    config::ConfigLock,
    db::DbObj,
    errors::CodedError,
    provers::{ProverError, ProverObj},
    requestor_monitor::{AllowRequestors, PriorityRequestors},
    storage::{upload_image_uri, upload_input_uri},
    task::{RetryRes, RetryTask, SupervisorErr},
    utils, FulfillmentType, OrderRequest, OrderStateChange,
};
use crate::{
    now_timestamp,
    provers::{ExecutorResp, ProofResult},
};
use alloy::{
    network::Ethereum,
    primitives::{
        utils::{format_ether, format_units, parse_ether, parse_units},
        Address, U256,
    },
    providers::{Provider, WalletProvider},
    uint,
};
use anyhow::{Context, Result};
use boundless_market::{
    contracts::{
        boundless_market::BoundlessMarketService, FulfillmentData, Predicate, PredicateType,
        RequestError, RequestInputType,
    },
    selector::{is_blake3_groth16_selector, SupportedSelectors},
};
use moka::future::Cache;
use thiserror::Error;
use tokio::sync::{broadcast, mpsc, Mutex};
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;

use OrderPricingOutcome::{Lock, ProveAfterLockExpire, Skip};

const MIN_CAPACITY_CHECK_INTERVAL: Duration = Duration::from_secs(5);

const ONE_MILLION: U256 = uint!(1_000_000_U256);

/// Maximum number of orders to cache for deduplication
const ORDER_DEDUP_CACHE_SIZE: u64 = 5000;

/// In-memory LRU cache for order deduplication by ID (prevents duplicate order processing)
type OrderCache = Arc<Cache<String, ()>>;

/// Configuration for preflight result caching
const PREFLIGHT_CACHE_SIZE: u64 = 5000;
const PREFLIGHT_CACHE_TTL_SECS: u64 = 3 * 60 * 60; // 3 hours

/// Cache for preflight results to avoid duplicate computations
type PreflightCache = Arc<Cache<PreflightCacheKey, PreflightCacheValue>>;

#[derive(Debug, Clone)]
enum ProveLimitReason {
    CollateralPricing { mcycle_price_collateral: String, collateral_reward: String },
    EthPricing { max_price: U256, gas_cost: U256, mcycle_price_eth: U256 },
    ConfigCap { max_mcycles: u64 },
    DeadlineCap { time_remaining_secs: u64, peak_prove_khz: u64 },
}

impl std::fmt::Display for ProveLimitReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProveLimitReason::CollateralPricing { mcycle_price_collateral, collateral_reward } => {
                write!(
                    f,
                    "collateral pricing: order collateral reward {} / {} mcycle_price_collateral_token config",
                    collateral_reward,
                    mcycle_price_collateral,
                )
            }
            ProveLimitReason::EthPricing { max_price, gas_cost, mcycle_price_eth } => {
                write!(
                    f,
                    "ETH pricing: (order maxPrice {} - gas {}) / mcycle_price config ({} per Mcycle)",
                    format_ether(*max_price),
                    format_ether(*gas_cost),
                    format_ether(*mcycle_price_eth)
                )
            }
            ProveLimitReason::ConfigCap { max_mcycles } => {
                write!(f, "broker max_mcycle_limit config setting ({} Mcycles)", max_mcycles)
            }
            ProveLimitReason::DeadlineCap { time_remaining_secs, peak_prove_khz } => {
                write!(
                    f,
                    "deadline: {}s remaining with peak_prove_khz config ({} kHz)",
                    time_remaining_secs, peak_prove_khz
                )
            }
        }
    }
}

#[derive(Error, Debug, Clone)]
#[non_exhaustive]
pub enum OrderPickerErr {
    #[error("{code} failed to fetch / push input: {0:#}", code = self.code())]
    FetchInputErr(#[source] Arc<anyhow::Error>),

    #[error("{code} failed to fetch / push image: {0:#}", code = self.code())]
    FetchImageErr(#[source] Arc<anyhow::Error>),

    #[error("{code} invalid request: {0}", code = self.code())]
    RequestError(Arc<RequestError>),

    #[error("{code} RPC error: {0:#}", code = self.code())]
    RpcErr(Arc<anyhow::Error>),

    #[error("{code} Unexpected error: {0:#}", code = self.code())]
    UnexpectedErr(Arc<anyhow::Error>),
}

impl CodedError for OrderPickerErr {
    fn code(&self) -> &str {
        match self {
            OrderPickerErr::FetchInputErr(_) => "[B-OP-001]",
            OrderPickerErr::FetchImageErr(_) => "[B-OP-002]",
            OrderPickerErr::RequestError(_) => "[B-OP-004]",
            OrderPickerErr::RpcErr(_) => "[B-OP-005]",
            OrderPickerErr::UnexpectedErr(_) => "[B-OP-500]",
        }
    }
}

impl From<anyhow::Error> for OrderPickerErr {
    fn from(err: anyhow::Error) -> Self {
        OrderPickerErr::UnexpectedErr(Arc::new(err))
    }
}

impl From<RequestError> for OrderPickerErr {
    fn from(err: RequestError) -> Self {
        OrderPickerErr::RequestError(Arc::new(err))
    }
}

#[derive(Clone)]
pub struct OrderPicker<P> {
    db: DbObj,
    config: ConfigLock,
    prover: ProverObj,
    provider: Arc<P>,
    chain_monitor: Arc<ChainMonitorService<P>>,
    market: BoundlessMarketService<Arc<P>>,
    supported_selectors: SupportedSelectors,
    // TODO ideal not to wrap in mutex, but otherwise would require supervisor refactor, try to find alternative
    new_order_rx: Arc<Mutex<mpsc::Receiver<Box<OrderRequest>>>>,
    priced_orders_tx: mpsc::Sender<Box<OrderRequest>>,
    collateral_token_decimals: u8,
    order_cache: OrderCache,
    preflight_cache: PreflightCache,
    order_state_tx: broadcast::Sender<OrderStateChange>,
    priority_requestors: PriorityRequestors,
    allow_requestors: AllowRequestors,
}

#[derive(Debug)]
#[non_exhaustive]
enum OrderPricingOutcome {
    // Order should be locked and proving commence after lock is secured
    Lock {
        total_cycles: u64,
        target_timestamp_secs: u64,
        // TODO handle checking what time the lock should occur before, when estimating proving time.
        expiry_secs: u64,
        target_mcycle_price: U256,
        max_mcycle_price: U256,
        config_min_mcycle_price: U256,
        current_mcycle_price: U256,
    },
    // Do not lock the order, but consider proving and fulfilling it after the lock expires
    ProveAfterLockExpire {
        total_cycles: u64,
        lock_expire_timestamp_secs: u64,
        expiry_secs: u64,
        mcycle_price: U256,
        config_min_mcycle_price: U256,
    },
    // Do not accept engage order
    Skip {
        reason: String,
    },
}

impl<P> OrderPicker<P>
where
    P: Provider<Ethereum> + 'static + Clone + WalletProvider,
{
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        db: DbObj,
        config: ConfigLock,
        prover: ProverObj,
        market_addr: Address,
        provider: Arc<P>,
        chain_monitor: Arc<ChainMonitorService<P>>,
        new_order_rx: mpsc::Receiver<Box<OrderRequest>>,
        order_result_tx: mpsc::Sender<Box<OrderRequest>>,
        collateral_token_decimals: u8,
        order_state_tx: broadcast::Sender<OrderStateChange>,
        priority_requestors: PriorityRequestors,
        allow_requestors: AllowRequestors,
    ) -> Self {
        let market = BoundlessMarketService::new_for_broker(
            market_addr,
            provider.clone(),
            provider.default_signer_address(),
        );

        Self {
            db,
            config,
            prover,
            provider,
            chain_monitor,
            market,
            supported_selectors: SupportedSelectors::default(),
            new_order_rx: Arc::new(Mutex::new(new_order_rx)),
            priced_orders_tx: order_result_tx,
            collateral_token_decimals,
            order_cache: Arc::new(
                Cache::builder()
                    .max_capacity(ORDER_DEDUP_CACHE_SIZE)
                    .time_to_live(Duration::from_secs(60 * 60)) // 1 hour
                    .build(),
            ),
            preflight_cache: Arc::new(
                Cache::builder()
                    .max_capacity(PREFLIGHT_CACHE_SIZE)
                    .time_to_live(Duration::from_secs(PREFLIGHT_CACHE_TTL_SECS))
                    .build(),
            ),
            order_state_tx,
            priority_requestors,
            allow_requestors,
        }
    }

    async fn price_order_and_update_state(
        &self,
        mut order: Box<OrderRequest>,
        cancel_token: CancellationToken,
    ) -> bool {
        let order_id = order.id();
        let f = || async {
            let pricing_result = tokio::select! {
                result = self.price_order(&mut order) => result,
                _ = cancel_token.cancelled() => {
                    tracing::info!("Order pricing cancelled during pricing for order {order_id}");

                    // Add the cancelled order to the database as skipped
                    if let Err(e) = self.db.insert_skipped_request(&order).await {
                        tracing::error!("Failed to add cancelled order to database: {e}");
                    }
                    return Ok(false);
                }
            };

            match pricing_result {
                Ok(Lock {
                    total_cycles,
                    target_timestamp_secs,
                    expiry_secs,
                    target_mcycle_price,
                    max_mcycle_price,
                    current_mcycle_price,
                    config_min_mcycle_price,
                }) => {
                    order.total_cycles = Some(total_cycles);
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

                    self.priced_orders_tx
                        .send(order)
                        .await
                        .context("Failed to send to order_result_tx")?;

                    Ok::<_, OrderPickerErr>(true)
                }
                Ok(ProveAfterLockExpire {
                    total_cycles,
                    lock_expire_timestamp_secs,
                    expiry_secs,
                    mcycle_price,
                    config_min_mcycle_price,
                }) => {
                    tracing::info!("Setting order {order_id} to prove after lock expiry at {lock_expire_timestamp_secs} (projected price: {} ZKC/Mcycle, config min price: {} ZKC/Mcycle)", self.format_collateral(mcycle_price), self.format_collateral(config_min_mcycle_price));
                    order.total_cycles = Some(total_cycles);
                    order.target_timestamp = Some(lock_expire_timestamp_secs);
                    order.expire_timestamp = Some(expiry_secs);

                    self.priced_orders_tx
                        .send(order)
                        .await
                        .context("Failed to send to order_result_tx")?;

                    Ok(true)
                }
                Ok(Skip { reason }) => {
                    tracing::info!("Skipping order {order_id}: {reason}");

                    // Add the skipped order to the database
                    self.db
                        .insert_skipped_request(&order)
                        .await
                        .context("Failed to add skipped order to database")?;
                    Ok(false)
                }
                Err(err) => {
                    tracing::warn!("Failed to price order {order_id}: {err}");
                    self.db
                        .insert_skipped_request(&order)
                        .await
                        .context("Failed to skip failed priced order")?;
                    Ok(false)
                }
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

    async fn price_order(
        &self,
        order: &mut OrderRequest,
    ) -> Result<OrderPricingOutcome, OrderPickerErr> {
        let order_id = order.id();
        tracing::debug!("Pricing order {order_id}");

        let now = now_timestamp();

        // If order_expiration > lock_expiration the period in-between is when order can be filled
        // by anyone without staking to partially claim the slashed collateral
        let lock_expired = order.fulfillment_type == FulfillmentType::FulfillAfterLockExpire;

        let expiration = order.expiry();
        let lockin_collateral =
            if lock_expired { U256::ZERO } else { U256::from(order.request.offer.lockCollateral) };

        if expiration <= now {
            return Ok(Skip { reason: "order has expired".to_string() });
        };

        let (min_deadline, denied_addresses_opt, min_mcycle_limit) = {
            let config = self.config.lock_all().context("Failed to read config")?;
            (
                config.market.min_deadline,
                config.market.deny_requestor_addresses.clone(),
                config.market.min_mcycle_limit,
            )
        };

        // Does the order expire within the min deadline
        let seconds_left = expiration.saturating_sub(now);
        if seconds_left <= min_deadline {
            return Ok(Skip {
                reason: format!(
                    "order expires in {seconds_left} seconds with min_deadline {min_deadline}"
                ),
            });
        }

        // Check if requestor is allowed (from both static config and dynamic lists)
        let client_addr = order.request.client_address();
        if !self.allow_requestors.is_allow_requestor(&client_addr) {
            // Only skip if allow list is actually configured (either static or dynamic)
            let has_allow_list = {
                let config = self.config.lock_all().context("Failed to read config")?;
                config.market.allow_client_addresses.is_some()
                    || config.market.allow_requestor_lists.is_some()
            };
            if has_allow_list {
                return Ok(Skip {
                    reason: format!("order from {client_addr} is not in allow requestors"),
                });
            }
        }

        if let Some(deny_addresses) = denied_addresses_opt {
            if deny_addresses.contains(&client_addr) {
                return Ok(Skip {
                    reason: format!(
                        "order is from {} and is in deny_requestor_addresses",
                        client_addr
                    ),
                });
            }
        }

        if !self.supported_selectors.is_supported(order.request.requirements.selector) {
            return Ok(Skip {
                reason: format!(
                    "unsupported selector requirement. Requested: {:x}. Supported: {:?}",
                    order.request.requirements.selector,
                    self.supported_selectors
                        .selectors
                        .iter()
                        .map(|(k, v)| format!("{k:x} ({v:?})"))
                        .collect::<Vec<_>>()
                ),
            });
        };

        // Check if the collateral is sane and if we can afford it
        // For lock expired orders, we don't check the max collateral because we can't lock those orders.
        let max_collateral: U256 = {
            let config = self.config.lock_all().context("Failed to read config")?;
            parse_units(&config.market.max_collateral, self.collateral_token_decimals)
                .context("Failed to parse max_collateral")?
                .into()
        };

        if !lock_expired && lockin_collateral > max_collateral {
            return Ok(Skip {
                reason: format!(
                    "order collateral requirement exceeds max_collateral config {} > {}",
                    self.format_collateral(lockin_collateral),
                    self.format_collateral(max_collateral),
                ),
            });
        }

        // Short circuit if the order has been locked.
        if order.fulfillment_type == FulfillmentType::LockAndFulfill
            && self
                .db
                .is_request_locked(U256::from(order.request.id))
                .await
                .context("Failed to check if request is locked before pricing")?
        {
            return Ok(Skip { reason: "order is already locked".to_string() });
        }

        if order.fulfillment_type == FulfillmentType::FulfillAfterLockExpire
            && self
                .db
                .is_request_fulfilled(U256::from(order.request.id))
                .await
                .context("Failed to check if request is fulfilled before pricing")?
        {
            return Ok(Skip { reason: "order is already fulfilled".to_string() });
        }

        // Check that we have both enough staking tokens to collateral, and enough gas tokens to lock and fulfil
        // NOTE: We use the current gas price and a rough heuristic on gas costs. Its possible that
        // gas prices may go up (or down) by the time its time to fulfill. This does not aim to be
        // a tight estimate, although improving this estimate will allow for a more profit.
        let gas_price =
            self.chain_monitor.current_gas_price().await.context("Failed to get gas price")?;
        let order_gas = if lock_expired {
            // No need to include lock gas if its a lock expired order
            U256::from(
                utils::estimate_gas_to_fulfill(
                    &self.config,
                    &self.supported_selectors,
                    &order.request,
                )
                .await?,
            )
        } else {
            U256::from(
                utils::estimate_gas_to_lock(&self.config, order).await?
                    + utils::estimate_gas_to_fulfill(
                        &self.config,
                        &self.supported_selectors,
                        &order.request,
                    )
                    .await?,
            )
        };
        let order_gas_cost = U256::from(gas_price) * order_gas;
        let available_gas = self.available_gas_balance().await?;
        let available_collateral = self.available_collateral_balance().await?;
        tracing::debug!(
            "Estimated {order_gas} gas to {} order {order_id}; {} ether @ {} gwei",
            if lock_expired { "fulfill" } else { "lock and fulfill" },
            format_ether(order_gas_cost),
            format_units(gas_price, "gwei").unwrap()
        );

        if order_gas_cost > order.request.offer.maxPrice && !lock_expired {
            // Cannot check the gas cost for lock expired orders where the reward is a fraction of the collateral
            // TODO: This can be added once we have a price feed for the collateral token in gas tokens
            return Ok(Skip {
                reason: format!(
                    "estimated gas cost to lock and fulfill order of {} exceeds max price of {}",
                    format_ether(order_gas_cost),
                    format_ether(order.request.offer.maxPrice)
                ),
            });
        }

        if order_gas_cost > available_gas {
            return Ok(Skip {
                reason: format!(
                    "estimated gas cost to lock and fulfill order of {} exceeds available gas of {}",
                    format_ether(order_gas_cost),
                    format_ether(available_gas)
                ),
            });
        }

        if !lock_expired && lockin_collateral > available_collateral {
            return Ok(Skip {
                reason: format!(
                    "insufficient available collateral to lock order. Requires {} but has {}",
                    self.format_collateral(lockin_collateral),
                    self.format_collateral(available_collateral),
                ),
            });
        }

        // Calculate exec limit (handles priority requestors and config internally)
        let (exec_limit_cycles, prove_limit, prove_limit_reason) =
            self.calculate_exec_limits(order, order_gas_cost)?;

        if prove_limit < 2 {
            // Exec limit is based on user cycles, and 2 is the minimum number of user cycles for a
            // provable execution.
            // TODO when/if total cycle limit is allowed in future, update this to be total cycle min
            return Ok(Skip {
                reason: format!("exec limit is too low for order of {}", prove_limit),
            });
        }

        tracing::debug!(
            "Starting preflight execution of {order_id} with limit of {} cycles (~{} mcycles)",
            exec_limit_cycles,
            exec_limit_cycles / 1_000_000
        );

        // Create cache key based on input type
        let predicate_data = order.request.requirements.predicate.data.to_vec();
        let cache_key = match order.request.input.inputType {
            RequestInputType::Url => {
                let input_url = std::str::from_utf8(&order.request.input.data)
                    .context("input url is not utf8")
                    .map_err(|e| OrderPickerErr::FetchInputErr(Arc::new(e)))?
                    .to_string();
                PreflightCacheKey { predicate_data, input: InputCacheKey::Url(input_url) }
            }
            RequestInputType::Inline => {
                // For inline inputs, use SHA256 hash of the data
                let mut hasher = Sha256::new();
                Sha2Digest::update(&mut hasher, &order.request.input.data);
                let input_hash: [u8; 32] = hasher.finalize().into();
                PreflightCacheKey { predicate_data, input: InputCacheKey::Hash(input_hash) }
            }
            RequestInputType::__Invalid => {
                return Err(OrderPickerErr::UnexpectedErr(Arc::new(anyhow::anyhow!(
                    "Unknown input type: {:?}",
                    order.request.input.inputType
                ))));
            }
        };

        // Loop while the cached result is skipped and has a lower exec limit than the current order.
        let preflight_result = loop {
            let prover = self.prover.clone();
            let config = self.config.clone();
            let request = order.request.clone();
            let order_id_clone = order_id.clone();
            let cache_key_clone: PreflightCacheKey = cache_key.clone();
            let priority_requestors = self.priority_requestors.clone();

            let cache_cloned = self.preflight_cache.clone();
            let result = tokio::task::spawn(async move {

                // Multiple concurrent calls of this coalesce into a single execution. This is done
                // to prevent multiple preflight jobs starting for the same program/input.
                // https://docs.rs/moka/latest/moka/sync/struct.Cache.html#concurrent-calls-on-the-same-key-2
                cache_cloned
                    .try_get_with(cache_key_clone, async move {
                        tracing::trace!(
                            "Starting preflight of {order_id_clone} with exec limit {exec_limit_cycles} mcycles",
                        );

                        // Upload image and input only if not cached
                        let image_id = upload_image_uri(&prover, &request, &config)
                            .await
                            .map_err(|e| OrderPickerErr::FetchImageErr(Arc::new(e)))?;

                        let input_id = upload_input_uri(&prover, &request, &config, &priority_requestors)
                            .await
                            .map_err(|e| OrderPickerErr::FetchInputErr(Arc::new(e)))?;

                        // TODO add a future timeout here to put a upper bound on how long to preflight for
                        match prover
                            .preflight(
                                &image_id,
                                &input_id,
                                vec![],
                                Some(exec_limit_cycles),
                                &order_id_clone,
                            )
                            .await
                        {
                            Ok(res) => {
                                tracing::debug!(
                                    "Preflight execution of {order_id_clone} with session id {} and {} mcycles completed in {} seconds",
                                    res.id,
                                    res.stats.total_cycles / 1_000_000,
                                    res.elapsed_time
                                );
                                Ok(PreflightCacheValue::Success {
                                    exec_session_id: res.id,
                                    cycle_count: res.stats.total_cycles,
                                    image_id,
                                    input_id,
                                })
                            }
                            Err(err) => match err {
                                ProverError::ProvingFailed(ref err_msg) => {
                                    if err_msg.contains("Session limit exceeded")
                                        || err_msg.contains("Execution stopped intentionally due to session limit") {
                                        tracing::debug!(
                                            "Skipping order {order_id_clone} due to intentional execution limit of {exec_limit_cycles}",
                                        );
                                        Ok(PreflightCacheValue::Skip {
                                            cached_limit: exec_limit_cycles,
                                        })
                                    } else if err_msg.contains("Guest panicked") || err_msg.contains("GuestPanic") {
                                        // Error message from bento and bonsai respectively for guest failures
                                        tracing::debug!("Skipping order {order_id_clone} due to guest panic (invalid request): {}", err_msg);
                                        Ok(PreflightCacheValue::Skip {
                                            // Use max cached limit, to avoid re-running preflight
                                            // for an invalid request.
                                            cached_limit: u64::MAX,
                                        })
                                    } else {
                                        Err(OrderPickerErr::UnexpectedErr(Arc::new(err.into())))
                                    }
                                }
                                _ => Err(OrderPickerErr::UnexpectedErr(Arc::new(err.into()))),
                            },
                        }
                    })
                    .await
            })
            .await
            .map_err(|e| OrderPickerErr::UnexpectedErr(Arc::new(e.into())))?;

            let cached_value = match result {
                Ok(value) => value,
                Err(e) => break Err((*e).clone()),
            };

            if let PreflightCacheValue::Skip { cached_limit } = cached_value {
                if cached_limit < exec_limit_cycles {
                    tracing::debug!(
                        "Cached result has insufficient limit for order {order_id} (cached: {}, required: {}), re-running preflight",
                        cached_limit, exec_limit_cycles
                    );
                    self.preflight_cache.invalidate(&cache_key).await;
                    continue;
                }
            }

            break Ok(cached_value);
        };

        // Handle the preflight result
        let (exec_session_id, cycle_count, image_id) = match preflight_result? {
            PreflightCacheValue::Success { exec_session_id, cycle_count, image_id, input_id } => {
                tracing::debug!(
                    "Using preflight result for {order_id}: session id {} with {} mcycles",
                    exec_session_id,
                    cycle_count / 1_000_000
                );

                // Update order with the uploaded IDs
                order.image_id = Some(image_id.clone());
                order.input_id = Some(input_id.clone());

                (exec_session_id, cycle_count, image_id)
            }
            PreflightCacheValue::Skip { .. } => {
                return Ok(Skip {
                    reason: format!(
                        "order preflight execution limit hit - limited by {prove_limit_reason}"
                    ),
                });
            }
        };

        let proof_res = ProofResult {
            id: exec_session_id,
            stats: ExecutorResp { total_cycles: cycle_count, ..Default::default() },
            elapsed_time: 0.0,
        };

        // If a max_mcycle_limit is configured check if the order is over that limit
        let proof_cycles = proof_res.stats.total_cycles;
        if proof_cycles > prove_limit {
            // If the preflight execution has completed, but for the variant is rejected,
            // provide the config value that needs to be updated in order to have accepted.
            let config_info = match &prove_limit_reason {
                ProveLimitReason::EthPricing { max_price, gas_cost, mcycle_price_eth } => {
                    let max_price_gas_adjusted = max_price.saturating_sub(*gas_cost);
                    let required_price_per_mcycle = max_price_gas_adjusted
                        .saturating_mul(ONE_MILLION)
                        / U256::from(proof_cycles);
                    let required_price_per_mcycle_ignore_gas =
                        max_price.saturating_mul(ONE_MILLION) / U256::from(proof_cycles);
                    format!(
                        "min_mcycle_price set to {} ETH/Mcycle in config, order requires min_mcycle_price <= {} ETH/Mcycle to be considered (gas cost: {} ETH, ignoring gas requires min {} ETH/Mcycle)",
                        format_ether(*mcycle_price_eth),
                        format_ether(required_price_per_mcycle),
                        format_ether(*gas_cost),
                        format_ether(required_price_per_mcycle_ignore_gas)
                    )
                }
                ProveLimitReason::CollateralPricing { mcycle_price_collateral, .. } => {
                    let reward =
                        order.request.offer.collateral_reward_if_locked_and_not_fulfilled();
                    let required_collateral_price =
                        reward.saturating_mul(ONE_MILLION) / U256::from(proof_cycles);
                    format!(
                        "min_mcycle_price_collateral_token set to {} ZKC/Mcycle in config, order requires min_mcycle_price_collateral_token <= {} ZKC/Mcycle to be considered",
                        mcycle_price_collateral,
                        self.format_collateral(required_collateral_price)
                    )
                }
                ProveLimitReason::ConfigCap { max_mcycles } => {
                    let required_mcycles = proof_cycles.div_ceil(1_000_000);
                    format!(
                        "max_mcycle_limit set to {} Mcycles in config, order requires max_mcycle_limit >= {} Mcycles to be considered",
                        max_mcycles, required_mcycles
                    )
                }
                ProveLimitReason::DeadlineCap { time_remaining_secs, peak_prove_khz } => {
                    let denom = time_remaining_secs.saturating_mul(1_000);
                    let required_khz = proof_cycles.div_ceil(denom);
                    format!(
                        "peak_prove_khz set to {} kHz in config, order requires peak_prove_khz >= {} kHz to be considered",
                        peak_prove_khz, required_khz
                    )
                }
            };

            return Ok(Skip {
                reason: format!(
                    "order with {proof_cycles} cycles above limit of {prove_limit} cycles - {config_info}"
                ),
            });
        }

        if min_mcycle_limit > 0 {
            let min_cycles = min_mcycle_limit.saturating_mul(1_000_000);
            if proof_cycles < min_cycles {
                tracing::debug!(
                        "Order {order_id} skipped due to min_mcycle_limit config: {} cycles < {} cycles",
                        proof_cycles,
                        min_cycles
                    );
                return Ok(Skip {
                        reason: format!(
                            "order with {} cycles below min limit of {} cycles - min_mcycle_limit set to {} Mcycles in config",
                            proof_cycles,
                            min_cycles,
                            min_mcycle_limit,
                        ),
                    });
            }
        }

        let journal = self
            .prover
            .get_preflight_journal(&proof_res.id)
            .await
            .context("Failed to fetch preflight journal")?
            .context("Failed to find preflight journal")?;

        // ensure the journal is a size we are willing to submit on-chain
        let max_journal_bytes =
            self.config.lock_all().context("Failed to read config")?.market.max_journal_bytes;
        let order_predicate_type = order.request.requirements.predicate.predicateType;
        if matches!(order_predicate_type, PredicateType::PrefixMatch | PredicateType::DigestMatch)
            && journal.len() > max_journal_bytes
        {
            return Ok(Skip {
                reason: format!(
                    "order journal larger than set limit ({} > {})",
                    journal.len(),
                    max_journal_bytes,
                ),
            });
        }

        // If the selector is a blake3 groth16 selector, ensure the journal is exactly 32 bytes
        if is_blake3_groth16_selector(order.request.requirements.selector) && journal.len() != 32 {
            tracing::info!(
                "Order {order_id} journal is not 32 bytes for blake3 groth16 selector, skipping",
            );
            return Ok(Skip {
                reason: "blake3 groth16 selector requires 32 byte journal".to_string(),
            });
        }

        // Validate the predicates:
        let predicate = Predicate::try_from(order.request.requirements.predicate.clone())
            .map_err(|e| OrderPickerErr::RequestError(Arc::new(e.into())))?;
        let eval_data = if is_blake3_groth16_selector(order.request.requirements.selector) {
            // These proofs must have no journal delivery because they cannot be authenticated on chain.
            FulfillmentData::None
        } else {
            FulfillmentData::from_image_id_and_journal(Digest::from_hex(image_id).unwrap(), journal)
        };
        if predicate.eval(&eval_data).is_none() {
            return Ok(Skip { reason: "order predicate check failed".to_string() });
        }

        self.evaluate_order(order, &proof_res, order_gas_cost, lock_expired).await
    }

    async fn evaluate_order(
        &self,
        order: &OrderRequest,
        proof_res: &ProofResult,
        order_gas_cost: U256,
        lock_expired: bool,
    ) -> Result<OrderPricingOutcome, OrderPickerErr> {
        if lock_expired {
            return self.evaluate_lock_expired_order(order, proof_res).await;
        } else {
            self.evaluate_lockable_order(order, proof_res, order_gas_cost).await
        }
    }

    /// Evaluate if a regular lockable order is worth picking based on the price and the configured min mcycle price
    async fn evaluate_lockable_order(
        &self,
        order: &OrderRequest,
        proof_res: &ProofResult,
        order_gas_cost: U256,
    ) -> Result<OrderPricingOutcome, OrderPickerErr> {
        let config_min_mcycle_price = {
            let config = self.config.lock_all().context("Failed to read config")?;
            parse_ether(&config.market.min_mcycle_price).context("Failed to parse mcycle_price")?
        };

        let order_id = order.id();
        let one_mill = U256::from(1_000_000);

        let mcycle_price_min = U256::from(order.request.offer.minPrice)
            .saturating_sub(order_gas_cost)
            .saturating_mul(one_mill)
            / U256::from(proof_res.stats.total_cycles);
        let mcycle_price_max = U256::from(order.request.offer.maxPrice)
            .saturating_sub(order_gas_cost)
            .saturating_mul(one_mill)
            / U256::from(proof_res.stats.total_cycles);

        tracing::debug!(
            "Order {order_id} price: {}-{} ETH, {}-{} ETH per mcycle, {} collateral required, {} ETH gas cost",
            format_ether(U256::from(order.request.offer.minPrice)),
            format_ether(U256::from(order.request.offer.maxPrice)),
            format_ether(mcycle_price_min),
            format_ether(mcycle_price_max),
           self.format_collateral(order.request.offer.lockCollateral),
            format_ether(order_gas_cost),
        );

        // Skip the order if it will never be worth it
        if mcycle_price_max < config_min_mcycle_price {
            return Ok(Skip {
                reason: format!(
                    "order max price {} is less than mcycle_price config {}",
                    format_ether(U256::from(order.request.offer.maxPrice)),
                    format_ether(config_min_mcycle_price),
                ),
            });
        }

        let target_mcycle_price;
        let current_mcycle_price = order.request.offer.price_at(now_timestamp()).unwrap();
        let target_timestamp_secs = if mcycle_price_min >= config_min_mcycle_price {
            tracing::info!(
                "Selecting order {order_id} at price {} - ASAP",
                format_ether(current_mcycle_price)
            );
            target_mcycle_price = mcycle_price_min;
            0 // Schedule the lock ASAP
        } else {
            let target_min_price = config_min_mcycle_price
                .saturating_mul(U256::from(proof_res.stats.total_cycles))
                .div_ceil(ONE_MILLION)
                + order_gas_cost;
            tracing::debug!(
                "Order {order_id} minimum profitable price: {} ETH",
                format_ether(target_min_price)
            );

            target_mcycle_price = target_min_price;
            order
                .request
                .offer
                .time_at_price(target_min_price)
                .context("Failed to get target price timestamp")?
        };

        let expiry_secs = order.request.offer.rampUpStart + order.request.offer.lockTimeout as u64;

        Ok(Lock {
            total_cycles: proof_res.stats.total_cycles,
            target_timestamp_secs,
            expiry_secs,
            target_mcycle_price,
            max_mcycle_price: mcycle_price_max,
            current_mcycle_price,
            config_min_mcycle_price,
        })
    }

    /// Evaluate if a lock expired order is worth picking based on how much of the slashed collateral token we can recover
    /// and the configured min mcycle price in collateral tokens
    async fn evaluate_lock_expired_order(
        &self,
        order: &OrderRequest,
        proof_res: &ProofResult,
    ) -> Result<OrderPricingOutcome, OrderPickerErr> {
        let config_min_mcycle_price_collateral_tokens: U256 = {
            let config = self.config.lock_all().context("Failed to read config")?;
            parse_units(
                &config.market.min_mcycle_price_collateral_token,
                self.collateral_token_decimals,
            )
            .context("Failed to parse mcycle_price")?
            .into()
        };

        let total_cycles = U256::from(proof_res.stats.total_cycles);

        // Reward for the order is a fraction of the collateral once the lock has expired
        let price = order.request.offer.collateral_reward_if_locked_and_not_fulfilled();
        let mcycle_price_in_collateral_tokens = price.saturating_mul(ONE_MILLION) / total_cycles;

        tracing::debug!(
            "Order price: {} (collateral tokens) - cycles: {} - mcycle price: {} (collateral tokens), config_min_mcycle_price_collateral_tokens: {} (collateral tokens)",
            format_ether(price),
            proof_res.stats.total_cycles,
            self.format_collateral(mcycle_price_in_collateral_tokens),
            self.format_collateral(config_min_mcycle_price_collateral_tokens),
        );

        // Skip the order if it will never be worth it
        if mcycle_price_in_collateral_tokens < config_min_mcycle_price_collateral_tokens {
            return Ok(Skip {
                reason: format!(
                    "slashed collateral reward too low. {} (collateral reward) < config mcycle_price_collateral_token {}",
                    self.format_collateral(mcycle_price_in_collateral_tokens),
                    self.format_collateral(config_min_mcycle_price_collateral_tokens),
                ),
            });
        }

        Ok(ProveAfterLockExpire {
            total_cycles: proof_res.stats.total_cycles,
            lock_expire_timestamp_secs: order.request.offer.rampUpStart
                + order.request.offer.lockTimeout as u64,
            expiry_secs: order.request.offer.rampUpStart + order.request.offer.timeout as u64,
            mcycle_price: mcycle_price_in_collateral_tokens,
            config_min_mcycle_price: config_min_mcycle_price_collateral_tokens,
        })
    }

    /// Estimate of gas for fulfilling any orders either pending lock or locked
    async fn estimate_gas_to_fulfill_pending(&self) -> Result<u64> {
        let mut gas = 0;
        for order in self.db.get_committed_orders().await? {
            let gas_estimate = utils::estimate_gas_to_fulfill(
                &self.config,
                &self.supported_selectors,
                &order.request,
            )
            .await?;
            gas += gas_estimate;
        }
        tracing::debug!("Total gas estimate to fulfill pending orders: {}", gas);
        Ok(gas)
    }

    /// Estimate the total gas tokens reserved to lock and fulfill all pending orders
    async fn gas_balance_reserved(&self) -> Result<U256> {
        let gas_price =
            self.chain_monitor.current_gas_price().await.context("Failed to get gas price")?;
        let fulfill_pending_gas = self.estimate_gas_to_fulfill_pending().await?;
        Ok(U256::from(gas_price) * U256::from(fulfill_pending_gas))
    }

    /// Return available gas balance.
    ///
    /// This is defined as the balance of the signer account.
    async fn available_gas_balance(&self) -> Result<U256, OrderPickerErr> {
        let balance = self
            .provider
            .get_balance(self.provider.default_signer_address())
            .await
            .map_err(|err| OrderPickerErr::RpcErr(Arc::new(err.into())))?;

        let gas_balance_reserved = self.gas_balance_reserved().await?;

        let available = balance.saturating_sub(gas_balance_reserved);
        tracing::debug!(
            "available gas balance: (account_balance) {} - (expected_future_gas) {} = {}",
            format_ether(balance),
            format_ether(gas_balance_reserved),
            format_ether(available)
        );

        Ok(available)
    }

    /// Return available collateral balance.
    ///
    /// This is defined as the balance in staking tokens of the signer account minus any pending locked collateral.
    async fn available_collateral_balance(&self) -> Result<U256> {
        let balance =
            self.market.balance_of_collateral(self.provider.default_signer_address()).await?;
        Ok(balance)
    }

    /// Calculates the cycle limit for the preflight and also for the max cycles that this specific
    /// order variant will consider proving for.
    ///
    /// The reason for calculating both preflight and prove limits is to execute the order with
    /// a large enough cycle limit for both lock and fulfill orders as well as for if the order
    /// expires and to prove after lock expiry so that the execution can be cached and only happen
    /// once. The prove limit is the limit for this specific order variant and decides the max
    /// cycles the order can be for the prover to decide to commit to proving it.
    fn calculate_exec_limits(
        &self,
        order: &OrderRequest,
        order_gas_cost: U256,
    ) -> Result<(u64, u64, ProveLimitReason), OrderPickerErr> {
        // Derive parameters from order
        let order_id = order.id();
        let is_fulfill_after_lock_expire =
            order.fulfillment_type == FulfillmentType::FulfillAfterLockExpire;
        let now = now_timestamp();
        let request_expiration = order.expiry();
        let lock_expiry = order.request.lock_expires_at();
        let order_expiry = order.request.expires_at();
        let (max_mcycle_limit, peak_prove_khz, min_mcycle_price, min_mcycle_price_collateral_token) = {
            let config = self.config.lock_all().context("Failed to read config")?;
            (
                config.market.max_mcycle_limit,
                config.market.peak_prove_khz,
                parse_ether(&config.market.min_mcycle_price)
                    .context("Failed to parse mcycle_price")?,
                parse_units(
                    &config.market.min_mcycle_price_collateral_token,
                    self.collateral_token_decimals,
                )
                .context("Failed to parse mcycle_price")?
                .into(),
            )
        };

        // Pricing based cycle limits: Calculate the cycle limit based on collateral price
        let collateral_based_limit = if min_mcycle_price_collateral_token == U256::ZERO {
            tracing::warn!("min_mcycle_price_collateral_token is 0, setting unlimited exec limit");
            u64::MAX
        } else {
            let price = order.request.offer.collateral_reward_if_locked_and_not_fulfilled();

            let initial_collateral_based_limit =
                (price.saturating_mul(ONE_MILLION).div_ceil(min_mcycle_price_collateral_token))
                    .try_into()
                    .unwrap_or(u64::MAX);

            tracing::trace!(
                "Order {order_id} initial collateral based limit: {initial_collateral_based_limit}"
            );
            initial_collateral_based_limit
        };

        let mut preflight_limit = collateral_based_limit;
        let mut prove_limit = collateral_based_limit;
        let mut prove_limit_reason = ProveLimitReason::CollateralPricing {
            mcycle_price_collateral: self.format_collateral(min_mcycle_price_collateral_token),
            collateral_reward: self.format_collateral(
                order.request.offer.collateral_reward_if_locked_and_not_fulfilled(),
            ),
        };

        // If lock and fulfill, potentially increase that to ETH-based value if higher
        if !is_fulfill_after_lock_expire {
            // Calculate eth-based limit for lock and fulfill orders
            let eth_based_limit = if min_mcycle_price == U256::ZERO {
                tracing::warn!("min_mcycle_price is 0, setting unlimited exec limit");
                u64::MAX
            } else {
                (U256::from(order.request.offer.maxPrice)
                    .saturating_sub(order_gas_cost)
                    .saturating_mul(ONE_MILLION)
                    / min_mcycle_price)
                    .try_into()
                    .unwrap_or(u64::MAX)
            };

            if eth_based_limit > collateral_based_limit {
                // Eth based limit is higher, use that for both preflight and prove
                tracing::debug!("Order {order_id} eth based limit ({eth_based_limit}) > collateral based limit ({collateral_based_limit}), using eth based limit for both preflight and prove");
                preflight_limit = eth_based_limit;
                prove_limit = eth_based_limit;
            } else {
                // Otherwise lower the prove cycle limit for this order variant
                tracing::debug!("Order {order_id} eth based limit ({eth_based_limit}) < collateral based limit ({collateral_based_limit}), using eth based limit for prove");
                prove_limit = eth_based_limit;
            }
            prove_limit_reason = ProveLimitReason::EthPricing {
                max_price: U256::from(order.request.offer.maxPrice),
                gas_cost: order_gas_cost,
                mcycle_price_eth: min_mcycle_price,
            };
        }

        debug_assert!(
            preflight_limit >= prove_limit,
            "preflight_limit ({preflight_limit}) < prove_limit ({prove_limit})",
        );

        // Apply max mcycle limit cap
        // Check if priority requestor address - skip all exec limit calculations
        let client_addr = order.request.client_address();
        let skip_mcycle_limit = self.priority_requestors.is_priority_requestor(&client_addr);
        if skip_mcycle_limit {
            tracing::debug!("Order {order_id} exec limit config ignored due to client {} being part of priority requestors.", client_addr);
        }

        if !skip_mcycle_limit {
            let config_cycle_limit = max_mcycle_limit.saturating_mul(1_000_000);
            if prove_limit > config_cycle_limit {
                tracing::debug!(
                    "Order {order_id} prove limit capped by max_mcycle_limit config: {} -> {} cycles",
                    prove_limit,
                    config_cycle_limit
                );
                prove_limit = config_cycle_limit;
                preflight_limit = config_cycle_limit;
                prove_limit_reason = ProveLimitReason::ConfigCap { max_mcycles: max_mcycle_limit };
            } else if preflight_limit > config_cycle_limit {
                preflight_limit = config_cycle_limit;
            }
        }

        // Apply timing constraints based on peak prove khz
        if let Some(peak_prove_khz) = peak_prove_khz {
            let prove_window = request_expiration.saturating_sub(now);
            let prove_deadline_limit = calculate_max_cycles_for_time(peak_prove_khz, prove_window);
            if prove_limit > prove_deadline_limit {
                tracing::debug!("Order {order_id} prove limit capped by deadline: {} -> {} cycles ({:.1}s at {} peak_prove_khz)", prove_limit, prove_deadline_limit, prove_window, peak_prove_khz);
                prove_limit = prove_deadline_limit;
                prove_limit_reason = ProveLimitReason::DeadlineCap {
                    time_remaining_secs: prove_window,
                    peak_prove_khz,
                };
            }

            // For preflight, also check fulfill-after-expiry window
            let new_preflight_limit = if !is_fulfill_after_lock_expire {
                let fulfill_after_expiry_window = order_expiry.saturating_sub(lock_expiry);
                let fulfill_after_expiry_limit =
                    calculate_max_cycles_for_time(peak_prove_khz, fulfill_after_expiry_window);
                std::cmp::max(prove_deadline_limit, fulfill_after_expiry_limit)
            } else {
                prove_deadline_limit
            };

            if preflight_limit > new_preflight_limit {
                tracing::debug!("Order {order_id} preflight limit capped by deadline: {} -> {} cycles ({:.1}s at {} peak_prove_khz)", preflight_limit, new_preflight_limit, prove_window, peak_prove_khz);
                preflight_limit = new_preflight_limit;
            }
        }

        tracing::trace!(
            "Order {order_id} final limits - preflight: {} cycles, prove: {} cycles (reason: {})",
            preflight_limit,
            prove_limit,
            prove_limit_reason
        );

        debug_assert!(
            preflight_limit >= prove_limit,
            "preflight_limit ({preflight_limit}) < prove_limit ({prove_limit})",
        );

        Ok((preflight_limit, prove_limit, prove_limit_reason))
    }

    fn format_collateral(&self, collateral: U256) -> String {
        format_units(collateral, self.collateral_token_decimals).unwrap_or_else(|_| "?".to_string())
    }
}

/// Input type for preflight cache
#[derive(Hash, Eq, PartialEq, Clone, Debug)]
enum InputCacheKey {
    Url(String),
    Hash([u8; 32]),
}

/// Key type for the preflight cache
#[derive(Hash, Eq, PartialEq, Clone, Debug)]
struct PreflightCacheKey {
    predicate_data: Vec<u8>,
    input: InputCacheKey,
}

/// Value type for the preflight cache
#[derive(Clone, Debug)]
enum PreflightCacheValue {
    Success { exec_session_id: String, cycle_count: u64, image_id: String, input_id: String },
    Skip { cached_limit: u64 },
}

/// Handles a lock event for a request
/// Cancels and removes only LockAndFulfill orders
#[allow(clippy::vec_box)]
fn handle_lock_event(
    request_id: U256,
    active_tasks: &mut BTreeMap<U256, BTreeMap<String, CancellationToken>>,
    pending_orders: &mut Vec<Box<OrderRequest>>,
) {
    // Cancel only LockAndFulfill active tasks
    if let Some(order_tasks) = active_tasks.get_mut(&request_id) {
        let initial_count = order_tasks.len();
        order_tasks.retain(|order_id, task_token| {
            if order_id.contains("LockAndFulfill") {
                task_token.cancel();
                false
            } else {
                true
            }
        });
        let cancelled = initial_count - order_tasks.len();

        if cancelled > 0 {
            tracing::debug!(
                "Cancelled {} LockAndFulfill preflights for locked request 0x{:x}",
                cancelled,
                request_id
            );
        }

        // Remove the entry if no tasks remain
        if order_tasks.is_empty() {
            active_tasks.remove(&request_id);
        }
    }

    // Remove only pending LockAndFulfill orders
    let initial_len = pending_orders.len();
    pending_orders.retain(|order| {
        let same_request = U256::from(order.request.id) == request_id;
        let is_lock_and_fulfill = order.fulfillment_type == FulfillmentType::LockAndFulfill;
        !(same_request && is_lock_and_fulfill)
    });
    let removed_orders = initial_len - pending_orders.len();

    if removed_orders > 0 {
        tracing::debug!(
            "Removed {} pending LockAndFulfill orders for locked request 0x{:x}",
            removed_orders,
            request_id
        );
    }
}

/// Handles a fulfill event for a request
/// Cancels and removes all orders for the request
#[allow(clippy::vec_box)]
fn handle_fulfill_event(
    request_id: U256,
    active_tasks: &mut BTreeMap<U256, BTreeMap<String, CancellationToken>>,
    pending_orders: &mut Vec<Box<OrderRequest>>,
) {
    // Cancel all active tasks
    if let Some(order_tasks) = active_tasks.remove(&request_id) {
        let count = order_tasks.len();
        tracing::debug!(
            "Cancelling {} active preflights for fulfilled request 0x{:x}",
            count,
            request_id
        );
        for (_, task_token) in order_tasks {
            task_token.cancel();
        }
    }

    // Remove all pending orders
    let initial_len = pending_orders.len();
    pending_orders.retain(|order| U256::from(order.request.id) != request_id);
    let removed_orders = initial_len - pending_orders.len();

    if removed_orders > 0 {
        tracing::debug!(
            "Removed {} pending orders for fulfilled request 0x{:x}",
            removed_orders,
            request_id
        );
    }
}

impl<P> RetryTask for OrderPicker<P>
where
    P: Provider<Ethereum> + 'static + Clone + WalletProvider,
{
    type Error = OrderPickerErr;
    fn spawn(&self, cancel_token: CancellationToken) -> RetryRes<Self::Error> {
        let picker = self.clone();

        Box::pin(async move {
            tracing::info!("Starting order picking monitor");

            let read_config = || -> Result<_, Self::Error> {
                let cfg = picker.config.lock_all().map_err(|err| {
                    OrderPickerErr::UnexpectedErr(Arc::new(anyhow::anyhow!(
                        "Failed to read config: {err}"
                    )))
                })?;
                Ok((
                    cfg.market.max_concurrent_preflights as usize,
                    cfg.market.order_pricing_priority,
                    cfg.market.priority_requestor_addresses.clone(),
                ))
            };

            let (mut current_capacity, mut priority_mode, mut priority_addresses) =
                read_config().map_err(SupervisorErr::Fault)?;
            let mut tasks: JoinSet<(String, U256)> = JoinSet::new();
            let mut rx = picker.new_order_rx.lock().await;
            let mut order_state_rx = picker.order_state_tx.subscribe();
            let mut capacity_check_interval = tokio::time::interval(MIN_CAPACITY_CHECK_INTERVAL);
            let mut pending_orders: Vec<Box<OrderRequest>> = Vec::new();
            let mut active_tasks: BTreeMap<U256, BTreeMap<String, CancellationToken>> =
                BTreeMap::new();
            let mut last_active_tasks_log: String = String::new();

            loop {
                tokio::select! {
                    // This channel is cancellation safe, so it's fine to use in the select!
                    Some(order) = rx.recv() => {
                        let order_id = order.id();
                        pending_orders.push(order);
                        tracing::debug!(
                            "Queued order {} to be priced. Currently {} queued pricing tasks: {}",
                            order_id,
                            pending_orders.len(),
                            format_truncated(pending_orders.iter().map(|o| o.id()))
                        );
                    }
                    Ok(state_change) = order_state_rx.recv() => {
                        match state_change {
                            OrderStateChange::Locked { request_id, prover } => {
                                tracing::debug!("Received order state change for request 0x{:x}: Locked by prover {:x}",
                                    request_id, prover);

                                handle_lock_event(request_id, &mut active_tasks, &mut pending_orders);
                            }
                            OrderStateChange::Fulfilled { request_id } => {
                                tracing::debug!("Received order state change for request 0x{:x}: Fulfilled",
                                    request_id);

                                handle_fulfill_event(request_id, &mut active_tasks, &mut pending_orders);
                            }
                        }
                    }
                    Some(result) = tasks.join_next(), if !tasks.is_empty() => {
                        if let Ok((order_id, request_id)) = result {
                            // Clean up the active task entry now that it's completed
                            if let Some(order_tasks) = active_tasks.get_mut(&request_id) {
                                order_tasks.remove(&order_id);
                                if order_tasks.is_empty() {
                                    active_tasks.remove(&request_id);
                                }
                            }


                            tracing::trace!("Priced task for order {} (request 0x{:x}) completed ({} remaining)",
                                order_id, request_id, tasks.len());
                        }
                    }
                    _ = capacity_check_interval.tick() => {
                        // Check capacity on an interval for capacity changes in config
                        let (new_capacity, new_priority_mode, new_priority_addresses) = read_config().map_err(SupervisorErr::Fault)?;
                        if new_capacity != current_capacity{
                            tracing::debug!("Pricing capacity changed from {} to {}", current_capacity, new_capacity);
                            current_capacity = new_capacity;
                        }
                        if new_priority_mode != priority_mode {
                            tracing::debug!("Order pricing priority changed from {:?} to {:?}", priority_mode, new_priority_mode);
                            priority_mode = new_priority_mode;
                        }
                        if new_priority_addresses != priority_addresses {
                            tracing::debug!("Priority requestor addresses changed");
                            priority_addresses = new_priority_addresses;
                        }

                        // Log active pricing tasks if they've changed
                        let current_tasks_log = format_active_tasks(&active_tasks);

                        if last_active_tasks_log != current_tasks_log {
                            tracing::debug!("Current pricing tasks: [{}]", current_tasks_log);
                            last_active_tasks_log = current_tasks_log;
                        }
                    }

                    _ = cancel_token.cancelled() => {
                        tracing::debug!("Order picker received cancellation, shutting down gracefully");

                        // Wait for all pricing tasks to be cancelled gracefully
                        while tasks.join_next().await.is_some() {}
                        break;
                    }
                }

                // Process pending orders if we have capacity
                if !pending_orders.is_empty() && tasks.len() < current_capacity {
                    let available_capacity = current_capacity - tasks.len();
                    let selected_orders = picker.select_pricing_orders(
                        &mut pending_orders,
                        priority_mode,
                        priority_addresses.as_deref(),
                        available_capacity,
                    );

                    for order in selected_orders {
                        let order_id = order.id();
                        let request_id = U256::from(order.request.id);

                        // Check if we're already processing this specific order
                        if let Some(order_tasks) = active_tasks.get(&request_id) {
                            if order_tasks.contains_key(&order_id) {
                                tracing::debug!(
                                    "Skipping order {order_id} - already being processed"
                                );
                                continue;
                            }
                        }

                        // Check if we've already started processing this order ID
                        if picker.order_cache.get(&order_id).await.is_some() {
                            tracing::debug!(
                                "Skipping duplicate order {order_id}, already being processed"
                            );
                            continue;
                        }

                        // Mark order as being processed immediately to prevent duplicates
                        picker.order_cache.insert(order_id.clone(), ()).await;

                        let picker_clone = picker.clone();
                        let task_cancel_token = cancel_token.child_token();

                        // Track the active task so it can be cancelled if needed
                        active_tasks
                            .entry(request_id)
                            .or_default()
                            .insert(order_id.clone(), task_cancel_token.clone());

                        tasks.spawn(async move {
                            picker_clone
                                .price_order_and_update_state(order, task_cancel_token)
                                .await;
                            (order_id, request_id)
                        });
                    }
                }
            }
            Ok(())
        })
    }
}

/// Format orders for logging, limiting to first 3 and showing total count
fn format_truncated<I, S>(mut iter: I) -> String
where
    I: Iterator<Item = S>,
    S: AsRef<str>,
{
    let first_three: Vec<String> = iter.by_ref().take(3).map(|s| s.as_ref().to_string()).collect();
    let remaining_count = iter.count();

    if remaining_count == 0 {
        first_three.join(", ")
    } else {
        format!("{}, ... ({} total)", first_three.join(", "), first_three.len() + remaining_count)
    }
}

/// Format active pricing tasks for logging, limiting to first 3 and showing total count
fn format_active_tasks(
    active_tasks: &BTreeMap<U256, BTreeMap<String, CancellationToken>>,
) -> String {
    format_truncated(active_tasks.values().flat_map(|orders| orders.keys()))
}

/// Returns the maximum cycles that can be proven within a given time period
/// based on the proving rate provided, in khz.
fn calculate_max_cycles_for_time(prove_khz: u64, time_seconds: u64) -> u64 {
    (prove_khz.saturating_mul(1_000)).saturating_mul(time_seconds)
}

#[cfg(test)]
pub(crate) mod tests {
    use std::time::Duration;

    use super::*;
    use crate::{
        chain_monitor::ChainMonitorService,
        db::SqliteDb,
        provers::{DefaultProver, Prover},
        FulfillmentType, OrderStatus,
    };
    use alloy::{
        network::EthereumWallet,
        node_bindings::{Anvil, AnvilInstance},
        primitives::{address, aliases::U96, utils::parse_units, Address, Bytes, FixedBytes},
        providers::{ext::AnvilApi, ProviderBuilder},
        signers::local::PrivateKeySigner,
    };
    use async_trait::async_trait;
    use boundless_market::storage::{MockStorageProvider, StorageProvider};
    use boundless_market::{
        contracts::{
            Callback, Offer, Predicate, ProofRequest, RequestId, RequestInput, Requirements,
        },
        selector::SelectorExt,
    };
    use boundless_test_utils::{
        guests::{ASSESSOR_GUEST_ID, ASSESSOR_GUEST_PATH, ECHO_ELF, ECHO_ID, LOOP_ELF, LOOP_ID},
        market::{deploy_boundless_market, deploy_hit_points},
    };
    use risc0_ethereum_contracts::selector::Selector;
    use risc0_zkvm::sha::Digest;
    use risc0_zkvm::Receipt;
    use tracing_test::traced_test;

    /// Reusable context for testing the order picker
    pub(crate) struct PickerTestCtx<P> {
        anvil: AnvilInstance,
        pub(crate) picker: OrderPicker<P>,
        boundless_market: BoundlessMarketService<Arc<P>>,
        storage_provider: MockStorageProvider,
        db: DbObj,
        provider: Arc<P>,
        priced_orders_rx: mpsc::Receiver<Box<OrderRequest>>,
        new_order_tx: mpsc::Sender<Box<OrderRequest>>,
    }

    /// Parameters for the generate_next_order function.
    pub(crate) struct OrderParams {
        pub(crate) order_index: u32,
        pub(crate) min_price: U256,
        pub(crate) max_price: U256,
        pub(crate) lock_collateral: U256,
        pub(crate) fulfillment_type: FulfillmentType,
        pub(crate) bidding_start: u64,
        pub(crate) lock_timeout: u32,
        pub(crate) timeout: u32,
    }

    impl Default for OrderParams {
        fn default() -> Self {
            Self {
                order_index: 1,
                min_price: parse_ether("0.02").unwrap(),
                max_price: parse_ether("0.04").unwrap(),
                lock_collateral: U256::ZERO,
                fulfillment_type: FulfillmentType::LockAndFulfill,
                bidding_start: now_timestamp(),
                lock_timeout: 900,
                timeout: 1200,
            }
        }
    }

    impl<P> PickerTestCtx<P>
    where
        P: Provider + WalletProvider,
    {
        pub(crate) fn signer(&self, index: usize) -> PrivateKeySigner {
            self.anvil.keys()[index].clone().into()
        }

        pub(crate) async fn generate_next_order(&self, params: OrderParams) -> Box<OrderRequest> {
            let image_url =
                self.storage_provider.upload_program(ECHO_ELF).await.unwrap().to_string();
            let image_id = Digest::from(ECHO_ID);
            let chain_id = self.provider.get_chain_id().await.unwrap();
            let boundless_market_address = self.boundless_market.instance().address();

            Box::new(OrderRequest {
                request: ProofRequest::new(
                    RequestId::new(self.provider.default_signer_address(), params.order_index),
                    Requirements::new(Predicate::prefix_match(image_id, Bytes::default())),
                    image_url,
                    RequestInput::builder()
                        .write_slice(&[0x41, 0x41, 0x41, 0x41])
                        .build_inline()
                        .unwrap(),
                    Offer {
                        minPrice: params.min_price,
                        maxPrice: params.max_price,
                        rampUpStart: params.bidding_start,
                        timeout: params.timeout,
                        lockTimeout: params.lock_timeout,
                        rampUpPeriod: 1,
                        lockCollateral: params.lock_collateral,
                    },
                ),
                target_timestamp: None,
                image_id: None,
                input_id: None,
                expire_timestamp: None,
                client_sig: Bytes::new(),
                fulfillment_type: params.fulfillment_type,
                boundless_market_address: *boundless_market_address,
                chain_id,
                total_cycles: None,
                cached_id: Default::default(),
            })
        }

        pub(crate) async fn generate_loop_order(
            &self,
            params: OrderParams,
            cycles: u64,
        ) -> Box<OrderRequest> {
            let image_url =
                self.storage_provider.upload_program(LOOP_ELF).await.unwrap().to_string();
            let image_id = Digest::from(LOOP_ID);
            let chain_id = self.provider.get_chain_id().await.unwrap();
            let boundless_market_address = self.boundless_market.instance().address();

            Box::new(OrderRequest {
                request: ProofRequest::new(
                    RequestId::new(self.provider.default_signer_address(), params.order_index),
                    Requirements::new(Predicate::prefix_match(image_id, Bytes::default())),
                    image_url,
                    RequestInput::builder()
                        .write(&cycles)
                        .unwrap()
                        .write(&1u64)
                        .unwrap() // nonce
                        .build_inline()
                        .unwrap(),
                    Offer {
                        minPrice: params.min_price,
                        maxPrice: params.max_price,
                        rampUpStart: params.bidding_start,
                        timeout: params.timeout,
                        lockTimeout: params.lock_timeout,
                        rampUpPeriod: 1,
                        lockCollateral: params.lock_collateral,
                    },
                ),
                target_timestamp: None,
                image_id: None,
                input_id: None,
                expire_timestamp: None,
                client_sig: Bytes::new(),
                fulfillment_type: params.fulfillment_type,
                boundless_market_address: *boundless_market_address,
                chain_id,
                total_cycles: None,
                cached_id: Default::default(),
            })
        }
    }

    #[derive(Default)]
    pub(crate) struct PickerTestCtxBuilder {
        initial_signer_eth: Option<i32>,
        initial_hp: Option<U256>,
        config: Option<ConfigLock>,
        collateral_token_decimals: Option<u8>,
        prover: Option<ProverObj>,
    }

    impl PickerTestCtxBuilder {
        pub(crate) fn with_initial_signer_eth(self, eth: i32) -> Self {
            Self { initial_signer_eth: Some(eth), ..self }
        }
        pub(crate) fn with_initial_hp(self, hp: U256) -> Self {
            assert!(hp < U256::from(U96::MAX), "Cannot have more than 2^96 hit points");
            Self { initial_hp: Some(hp), ..self }
        }
        pub(crate) fn with_config(self, config: ConfigLock) -> Self {
            Self { config: Some(config), ..self }
        }
        pub(crate) fn with_prover(self, prover: ProverObj) -> Self {
            Self { prover: Some(prover), ..self }
        }
        pub(crate) fn with_collateral_token_decimals(self, decimals: u8) -> Self {
            Self { collateral_token_decimals: Some(decimals), ..self }
        }
        pub(crate) async fn build(
            self,
        ) -> PickerTestCtx<impl Provider + WalletProvider + Clone + 'static> {
            let anvil = Anvil::new()
                .args(["--balance", &format!("{}", self.initial_signer_eth.unwrap_or(10000))])
                .spawn();
            let signer: PrivateKeySigner = anvil.keys()[0].clone().into();
            let provider = Arc::new(
                ProviderBuilder::new()
                    .wallet(EthereumWallet::from(signer.clone()))
                    .connect(&anvil.endpoint())
                    .await
                    .unwrap(),
            );

            provider.anvil_mine(Some(4), Some(2)).await.unwrap();

            let hp_contract = deploy_hit_points(signer.address(), provider.clone()).await.unwrap();
            let market_address = deploy_boundless_market(
                signer.address(),
                provider.clone(),
                address!("0x0000000000000000000000000000000000000001"),
                hp_contract,
                Digest::from(ASSESSOR_GUEST_ID),
                format!("file://{ASSESSOR_GUEST_PATH}"),
                Some(signer.address()),
            )
            .await
            .unwrap();

            let boundless_market = BoundlessMarketService::new_for_broker(
                market_address,
                provider.clone(),
                provider.default_signer_address(),
            );

            if let Some(initial_hp) = self.initial_hp {
                tracing::debug!("Setting initial locked hitpoints to {}", initial_hp);
                boundless_market.deposit_collateral_with_permit(initial_hp, &signer).await.unwrap();
                assert_eq!(
                    boundless_market
                        .balance_of_collateral(provider.default_signer_address())
                        .await
                        .unwrap(),
                    initial_hp
                );
            }

            let storage_provider = MockStorageProvider::start();

            let db: DbObj = Arc::new(SqliteDb::new("sqlite::memory:").await.unwrap());
            let config = self.config.unwrap_or_default();
            let prover: ProverObj = self.prover.unwrap_or_else(|| Arc::new(DefaultProver::new()));
            let chain_monitor = Arc::new(ChainMonitorService::new(provider.clone()).await.unwrap());
            tokio::spawn(chain_monitor.spawn(Default::default()));

            let chain_id = provider.get_chain_id().await.unwrap();
            let priority_requestors = PriorityRequestors::new(config.clone(), chain_id);
            let allow_requestors = AllowRequestors::new(config.clone(), chain_id);

            const TEST_CHANNEL_CAPACITY: usize = 50;
            let (_new_order_tx, new_order_rx) = mpsc::channel(TEST_CHANNEL_CAPACITY);
            let (priced_orders_tx, priced_orders_rx) = mpsc::channel(TEST_CHANNEL_CAPACITY);
            let (order_state_tx, _) = tokio::sync::broadcast::channel(TEST_CHANNEL_CAPACITY);

            let picker = OrderPicker::new(
                db.clone(),
                config,
                prover,
                market_address,
                provider.clone(),
                chain_monitor,
                new_order_rx,
                priced_orders_tx,
                self.collateral_token_decimals.unwrap_or(6),
                order_state_tx,
                priority_requestors,
                allow_requestors,
            );

            PickerTestCtx {
                anvil,
                picker,
                boundless_market,
                storage_provider,
                db,
                provider,
                priced_orders_rx,
                new_order_tx: _new_order_tx,
            }
        }
    }

    #[tokio::test]
    #[traced_test]
    async fn price_order() {
        let config = ConfigLock::default();
        {
            config.load_write().unwrap().market.min_mcycle_price = "0.0000001".into();
        }
        let mut ctx = PickerTestCtxBuilder::default().with_config(config).build().await;

        let order = ctx.generate_next_order(Default::default()).await;

        let _request_id =
            ctx.boundless_market.submit_request(&order.request, &ctx.signer(0)).await.unwrap();

        let locked = ctx.picker.price_order_and_update_state(order, CancellationToken::new()).await;
        assert!(locked);

        let priced_order = ctx.priced_orders_rx.try_recv().unwrap();
        assert_eq!(priced_order.target_timestamp, Some(0));
    }

    #[tokio::test]
    #[traced_test]
    async fn skip_bad_predicate() {
        let config = ConfigLock::default();
        {
            config.load_write().unwrap().market.min_mcycle_price = "0.0000001".into();
        }
        let ctx = PickerTestCtxBuilder::default().with_config(config).build().await;

        let mut order = ctx.generate_next_order(Default::default()).await;
        // set a bad predicate
        order.request.requirements.predicate =
            Predicate::digest_match(Digest::from(ECHO_ID), Digest::ZERO).into();

        let order_id = order.id();
        let _request_id =
            ctx.boundless_market.submit_request(&order.request, &ctx.signer(0)).await.unwrap();

        let locked = ctx.picker.price_order_and_update_state(order, CancellationToken::new()).await;
        assert!(!locked);

        let db_order = ctx.db.get_order(&order_id).await.unwrap().unwrap();
        assert_eq!(db_order.status, OrderStatus::Skipped);

        assert!(logs_contain("predicate check failed"));
    }

    #[tokio::test]
    #[traced_test]
    async fn skip_unsupported_selector() {
        let config = ConfigLock::default();
        {
            config.load_write().unwrap().market.min_mcycle_price = "0.0000001".into();
        }
        let ctx = PickerTestCtxBuilder::default().with_config(config).build().await;

        let mut order = ctx.generate_next_order(Default::default()).await;

        // set an unsupported selector
        order.request.requirements.selector = FixedBytes::from(Selector::Groth16V1_1 as u32);
        let order_id = order.id();

        let _request_id =
            ctx.boundless_market.submit_request(&order.request, &ctx.signer(0)).await.unwrap();

        let locked = ctx.picker.price_order_and_update_state(order, CancellationToken::new()).await;
        assert!(!locked);

        let db_order = ctx.db.get_order(&order_id).await.unwrap().unwrap();
        assert_eq!(db_order.status, OrderStatus::Skipped);

        assert!(logs_contain("unsupported selector requirement"));
    }

    #[tokio::test]
    #[traced_test]
    async fn skip_price_less_than_gas_costs() {
        let config = ConfigLock::default();
        {
            config.load_write().unwrap().market.min_mcycle_price = "0.0000001".into();
        }
        let ctx = PickerTestCtxBuilder::default().with_config(config).build().await;

        let order = ctx
            .generate_next_order(OrderParams {
                min_price: parse_ether("0.0005").unwrap(),
                max_price: parse_ether("0.0010").unwrap(),
                ..Default::default()
            })
            .await;
        let order_id = order.id();

        let _request_id =
            ctx.boundless_market.submit_request(&order.request, &ctx.signer(0)).await.unwrap();

        let locked = ctx.picker.price_order_and_update_state(order, CancellationToken::new()).await;
        assert!(!locked);

        let db_order = ctx.db.get_order(&order_id).await.unwrap().unwrap();
        assert_eq!(db_order.status, OrderStatus::Skipped);

        assert!(logs_contain("Estimated") && logs_contain("lock and fulfill order"));
    }

    #[tokio::test]
    #[traced_test]
    async fn skip_price_less_than_gas_costs_groth16() {
        let config = ConfigLock::default();
        {
            config.load_write().unwrap().market.min_mcycle_price = "0.0000001".into();
        }
        let mut ctx = PickerTestCtxBuilder::default().with_config(config).build().await;

        // NOTE: Values currently adjusted ad hoc to be between the two thresholds.
        let min_price = parse_ether("0.0013").unwrap();
        let max_price = parse_ether("0.0013").unwrap();

        // Order should have high enough price with the default selector.
        let order = ctx
            .generate_next_order(OrderParams {
                order_index: 1,
                min_price,
                max_price,
                ..Default::default()
            })
            .await;

        let _request_id =
            ctx.boundless_market.submit_request(&order.request, &ctx.signer(0)).await.unwrap();

        let locked = ctx.picker.price_order_and_update_state(order, CancellationToken::new()).await;
        assert!(locked);
        let priced = ctx.priced_orders_rx.try_recv().unwrap();
        assert_eq!(priced.target_timestamp, Some(0));

        // Order does not have high enough price when groth16 is used.
        let mut order = ctx
            .generate_next_order(OrderParams {
                order_index: 2,
                min_price,
                max_price,
                ..Default::default()
            })
            .await;

        // set a Groth16 selector
        order.request.requirements.selector =
            FixedBytes::from(SelectorExt::groth16_latest() as u32);

        let _request_id =
            ctx.boundless_market.submit_request(&order.request, &ctx.signer(0)).await.unwrap();

        let order_id = order.id();
        let locked = ctx.picker.price_order_and_update_state(order, CancellationToken::new()).await;
        assert!(!locked);

        let db_order = ctx.db.get_order(&order_id).await.unwrap().unwrap();
        assert_eq!(db_order.status, OrderStatus::Skipped);

        assert!(logs_contain("Estimated") && logs_contain("lock and fulfill order"));
    }

    #[tokio::test]
    #[traced_test]
    async fn skip_price_less_than_gas_costs_callback() {
        let config = ConfigLock::default();
        {
            config.load_write().unwrap().market.min_mcycle_price = "0.0000001".into();
        }
        let mut ctx = PickerTestCtxBuilder::default().with_config(config).build().await;

        // NOTE: Values currently adjusted ad hoc to be between the two thresholds.
        let min_price = parse_ether("0.0013").unwrap();
        let max_price = parse_ether("0.0013").unwrap();

        // Order should have high enough price with the default selector.
        let order = ctx
            .generate_next_order(OrderParams {
                order_index: 1,
                min_price,
                max_price,
                ..Default::default()
            })
            .await;
        let _request_id =
            ctx.boundless_market.submit_request(&order.request, &ctx.signer(0)).await.unwrap();

        let locked = ctx.picker.price_order_and_update_state(order, CancellationToken::new()).await;
        assert!(locked);

        let priced = ctx.priced_orders_rx.try_recv().unwrap();
        assert_eq!(priced.target_timestamp, Some(0));

        // Order does not have high enough price when groth16 is used.
        let mut order = ctx
            .generate_next_order(OrderParams {
                order_index: 2,
                min_price,
                max_price,
                ..Default::default()
            })
            .await;

        // set a callback with a nontrivial gas consumption
        order.request.requirements.callback = Callback {
            addr: address!("0x00000000000000000000000000000000ca11bac2"),
            gasLimit: U96::from(200_000),
        };

        let _request_id =
            ctx.boundless_market.submit_request(&order.request, &ctx.signer(0)).await.unwrap();

        let order_id = order.id();
        let locked = ctx.picker.price_order_and_update_state(order, CancellationToken::new()).await;
        assert!(!locked);

        let db_order = ctx.db.get_order(&order_id).await.unwrap().unwrap();
        assert_eq!(db_order.status, OrderStatus::Skipped);

        assert!(logs_contain("Estimated") && logs_contain("lock and fulfill order"));
    }

    #[tokio::test]
    #[traced_test]
    async fn skip_price_less_than_gas_costs_smart_contract_signature() {
        let config = ConfigLock::default();
        {
            config.load_write().unwrap().market.min_mcycle_price = "0.0000001".into();
        }
        let mut ctx = PickerTestCtxBuilder::default().with_config(config).build().await;

        // NOTE: Values currently adjusted ad hoc to be between the two thresholds.
        let min_price = parse_ether("0.00125").unwrap();
        let max_price = parse_ether("0.00125").unwrap();

        // Order should have high enough price with the default selector.
        let order = ctx
            .generate_next_order(OrderParams {
                order_index: 1,
                min_price,
                max_price,
                ..Default::default()
            })
            .await;

        let _request_id =
            ctx.boundless_market.submit_request(&order.request, &ctx.signer(0)).await.unwrap();

        let locked = ctx.picker.price_order_and_update_state(order, CancellationToken::new()).await;
        assert!(locked);

        let priced = ctx.priced_orders_rx.try_recv().unwrap();
        assert_eq!(priced.target_timestamp, Some(0));

        // Order does not have high enough price when groth16 is used.
        let mut order = ctx
            .generate_next_order(OrderParams {
                order_index: 2,
                min_price,
                max_price,
                ..Default::default()
            })
            .await;

        order.request.id =
            RequestId::try_from(order.request.id).unwrap().set_smart_contract_signed_flag().into();

        let _request_id =
            ctx.boundless_market.submit_request(&order.request, &ctx.signer(0)).await.unwrap();

        let order_id = order.id();
        let locked = ctx.picker.price_order_and_update_state(order, CancellationToken::new()).await;
        assert!(!locked);

        let db_order = ctx.db.get_order(&order_id).await.unwrap().unwrap();
        assert_eq!(db_order.status, OrderStatus::Skipped);

        assert!(logs_contain("Estimated") && logs_contain("lock and fulfill order"));
    }

    #[tokio::test]
    #[traced_test]
    async fn skip_unallowed_addr() {
        let config = ConfigLock::default();
        {
            config.load_write().unwrap().market.min_mcycle_price = "0.0000001".into();
            config.load_write().unwrap().market.allow_client_addresses = Some(vec![Address::ZERO]);
        }
        let ctx = PickerTestCtxBuilder::default().with_config(config).build().await;

        let order = ctx.generate_next_order(Default::default()).await;

        let _request_id =
            ctx.boundless_market.submit_request(&order.request, &ctx.signer(0)).await.unwrap();

        let order_id = order.id();
        let locked = ctx.picker.price_order_and_update_state(order, CancellationToken::new()).await;
        assert!(!locked);

        let db_order = ctx.db.get_order(&order_id).await.unwrap().unwrap();
        assert_eq!(db_order.status, OrderStatus::Skipped);

        assert!(logs_contain("is not in allow requestors"));
    }

    #[tokio::test]
    #[traced_test]
    async fn skip_denied_addr() {
        let config = ConfigLock::default();
        let ctx = PickerTestCtxBuilder::default().with_config(config.clone()).build().await;
        let deny_address = ctx.provider.default_signer_address();

        {
            let mut cfg = config.load_write().unwrap();
            cfg.market.min_mcycle_price = "0.0000001".into();
            cfg.market.deny_requestor_addresses = Some([deny_address].into_iter().collect());
        }

        let order = ctx.generate_next_order(Default::default()).await;

        let _request_id =
            ctx.boundless_market.submit_request(&order.request, &ctx.signer(0)).await.unwrap();

        let order_id = order.id();
        let locked = ctx.picker.price_order_and_update_state(order, CancellationToken::new()).await;
        assert!(!locked);

        let db_order = ctx.db.get_order(&order_id).await.unwrap().unwrap();
        assert_eq!(db_order.status, OrderStatus::Skipped);

        assert!(logs_contain("is in deny_requestor_addresses"));
    }

    #[tokio::test]
    #[traced_test]
    async fn resume_order_pricing() {
        let config = ConfigLock::default();
        {
            config.load_write().unwrap().market.min_mcycle_price = "0.0000001".into();
        }
        let mut ctx = PickerTestCtxBuilder::default().with_config(config).build().await;

        let order = ctx.generate_next_order(Default::default()).await;
        let order_id = order.id();

        let _request_id =
            ctx.boundless_market.submit_request(&order.request, &ctx.signer(0)).await.unwrap();

        let pricing_task = tokio::spawn(ctx.picker.spawn(Default::default()));

        ctx.new_order_tx.send(order).await.unwrap();

        // Wait for the order to be priced, with some timeout
        let priced_order =
            tokio::time::timeout(Duration::from_secs(10), ctx.priced_orders_rx.recv())
                .await
                .unwrap();
        assert_eq!(priced_order.unwrap().id(), order_id);

        pricing_task.abort();

        // Send a new order when picker task is down.
        let new_order = ctx.generate_next_order(Default::default()).await;
        let new_order_id = new_order.id();
        ctx.new_order_tx.send(new_order).await.unwrap();

        assert!(ctx.priced_orders_rx.is_empty());

        tokio::spawn(ctx.picker.spawn(Default::default()));

        let priced_order =
            tokio::time::timeout(Duration::from_secs(10), ctx.priced_orders_rx.recv())
                .await
                .unwrap();
        assert_eq!(priced_order.unwrap().id(), new_order_id);
    }

    #[tokio::test]
    #[traced_test]
    async fn cannot_overcommit_collateral() {
        let signer_inital_balance_eth = 2;
        let lockin_collateral = U256::from(150);

        let config = ConfigLock::default();
        {
            config.load_write().unwrap().market.min_mcycle_price = "0.0000001".into();
            config.load_write().unwrap().market.max_collateral = "10".into();
        }

        let mut ctx = PickerTestCtxBuilder::default()
            .with_initial_signer_eth(signer_inital_balance_eth)
            .with_initial_hp(lockin_collateral)
            .with_config(config)
            .build()
            .await;
        let order = ctx
            .generate_next_order(OrderParams {
                lock_collateral: U256::from(100),
                ..Default::default()
            })
            .await;
        let order1_id = order.id();
        assert!(ctx.picker.price_order_and_update_state(order, CancellationToken::new()).await);
        let priced = ctx.priced_orders_rx.try_recv().unwrap();
        assert_eq!(priced.id(), order1_id);

        let order = ctx
            .generate_next_order(OrderParams {
                lock_collateral: lockin_collateral + U256::from(1),
                ..Default::default()
            })
            .await;
        let order_id = order.id();
        assert!(!ctx.picker.price_order_and_update_state(order, CancellationToken::new()).await);
        assert!(logs_contain("insufficient available collateral to lock order"));
        assert_eq!(
            ctx.db.get_order(&order_id).await.unwrap().unwrap().status,
            OrderStatus::Skipped
        );

        let order = ctx
            .generate_next_order(OrderParams {
                lock_collateral: parse_units("11", ctx.picker.collateral_token_decimals)
                    .unwrap()
                    .into(),
                ..Default::default()
            })
            .await;
        let order_id = order.id();
        assert!(!ctx.picker.price_order_and_update_state(order, CancellationToken::new()).await);

        // only the first order above should have marked as active pricing, the second one should have been skipped due to insufficient collateral
        assert_eq!(
            ctx.db.get_order(&order_id).await.unwrap().unwrap().status,
            OrderStatus::Skipped
        );
        assert!(logs_contain("collateral requirement exceeds max_collateral config"));
    }

    #[tokio::test]
    #[traced_test]
    async fn use_gas_to_fulfill_estimate_from_config() {
        let fulfill_gas = 123_456;
        let config = ConfigLock::default();
        {
            config.load_write().unwrap().market.min_mcycle_price = "0.0000001".into();
            config.load_write().unwrap().market.fulfill_gas_estimate = fulfill_gas;
        }

        let mut ctx = PickerTestCtxBuilder::default().with_config(config).build().await;

        let order = ctx.generate_next_order(Default::default()).await;
        let locked = ctx.picker.price_order_and_update_state(order, CancellationToken::new()).await;
        assert!(locked);

        // Simulate order being locked
        let order = ctx.priced_orders_rx.try_recv().unwrap();
        ctx.db.insert_accepted_request(&order, order.request.offer.minPrice).await.unwrap();

        assert_eq!(ctx.picker.estimate_gas_to_fulfill_pending().await.unwrap(), fulfill_gas);

        // add another order
        let order =
            ctx.generate_next_order(OrderParams { order_index: 2, ..Default::default() }).await;
        let locked = ctx.picker.price_order_and_update_state(order, CancellationToken::new()).await;
        assert!(locked);
        let order = ctx.priced_orders_rx.try_recv().unwrap();
        ctx.db.insert_accepted_request(&order, order.request.offer.minPrice).await.unwrap();

        // gas estimate stacks (until estimates factor in bundling)
        assert_eq!(ctx.picker.estimate_gas_to_fulfill_pending().await.unwrap(), 2 * fulfill_gas);
    }

    #[tokio::test]
    #[traced_test]
    async fn skips_journal_exceeding_limit() {
        // set this by testing a very small limit (1 byte)
        let config = ConfigLock::default();
        {
            config.load_write().unwrap().market.min_mcycle_price = "0.0000001".into();
            config.load_write().unwrap().market.max_journal_bytes = 1;
        }
        let lock_collateral = U256::from(10);

        let ctx = PickerTestCtxBuilder::default()
            .with_config(config)
            .with_initial_hp(lock_collateral)
            .build()
            .await;
        let order =
            ctx.generate_next_order(OrderParams { lock_collateral, ..Default::default() }).await;

        let order_id = order.id();
        let locked = ctx.picker.price_order_and_update_state(order, CancellationToken::new()).await;
        assert!(!locked);

        assert_eq!(
            ctx.db.get_order(&order_id).await.unwrap().unwrap().status,
            OrderStatus::Skipped
        );
        assert!(logs_contain("journal larger than set limit"));
    }

    #[tokio::test]
    #[traced_test]
    async fn price_locked_by_other() {
        let config = ConfigLock::default();
        {
            config.load_write().unwrap().market.min_mcycle_price_collateral_token =
                "0.0000001".into();
        }
        let mut ctx = PickerTestCtxBuilder::default()
            .with_config(config)
            .with_initial_hp(U256::from(1000))
            .build()
            .await;

        let order = ctx
            .generate_next_order(OrderParams {
                fulfillment_type: FulfillmentType::FulfillAfterLockExpire,
                bidding_start: now_timestamp(),
                lock_timeout: 1000,
                timeout: 10000,
                lock_collateral: parse_units("0.1", 6).unwrap().into(),
                ..Default::default()
            })
            .await;

        let order_id = order.id();
        let expected_target_timestamp =
            order.request.offer.rampUpStart + order.request.offer.lockTimeout as u64;
        let expected_expire_timestamp =
            order.request.offer.rampUpStart + order.request.offer.timeout as u64;

        let expected_log = format!(
            "Setting order {order_id} to prove after lock expiry at {expected_target_timestamp}"
        );
        assert!(ctx.picker.price_order_and_update_state(order, CancellationToken::new()).await);

        assert!(logs_contain(&expected_log));

        let priced = ctx.priced_orders_rx.try_recv().unwrap();
        assert_eq!(priced.target_timestamp, Some(expected_target_timestamp));
        assert_eq!(priced.expire_timestamp, Some(expected_expire_timestamp));
    }

    #[tokio::test]
    #[traced_test]
    async fn price_locked_by_other_unprofitable() {
        let config = ConfigLock::default();
        {
            config.load_write().unwrap().market.min_mcycle_price_collateral_token = "0.1".into();
        }
        let ctx = PickerTestCtxBuilder::default()
            .with_collateral_token_decimals(6)
            .with_config(config)
            .build()
            .await;

        let order = ctx
            .generate_next_order(OrderParams {
                fulfillment_type: FulfillmentType::FulfillAfterLockExpire,
                bidding_start: now_timestamp(),
                lock_timeout: 0,
                timeout: 10000,
                // Low collateral means low reward for filling after it is unfulfilled
                lock_collateral: parse_units("0.00001", 6).unwrap().into(),
                ..Default::default()
            })
            .await;

        let order_id = order.id();

        assert!(!ctx.picker.price_order_and_update_state(order, CancellationToken::new()).await);

        // Since we know the collateral reward is constant, and we know our min_mycle_price_collateral_token
        // the execution limit check tells us if the order is profitable or not, since it computes the max number
        // of cycles that can be proven while keeping the order profitable.
        assert!(logs_contain(&format!(
            "Skipping order {order_id} due to intentional execution limit of"
        )));

        let db_order = ctx.db.get_order(&order_id).await.unwrap().unwrap();
        assert_eq!(db_order.status, OrderStatus::Skipped);
    }

    #[tokio::test]
    #[traced_test]
    async fn skip_mcycle_limit_for_allowed_address() {
        let exec_limit = 1000;
        let config = ConfigLock::default();
        {
            config.load_write().unwrap().market.min_mcycle_price = "0.0000001".into();
            config.load_write().unwrap().market.max_mcycle_limit = exec_limit;
        }
        let ctx = PickerTestCtxBuilder::default().with_config(config).build().await;

        ctx.picker.config.load_write().as_mut().unwrap().market.priority_requestor_addresses =
            Some(vec![ctx.provider.default_signer_address()]);

        // First order from allowed address - should skip mcycle limit
        let order = ctx.generate_next_order(Default::default()).await;
        let order_id = order.id();

        let locked = ctx.picker.price_order_and_update_state(order, CancellationToken::new()).await;
        assert!(locked);

        // Check logs for the expected message about skipping mcycle limit
        assert!(logs_contain(&format!(
            "Order {order_id} exec limit config ignored due to client {} being part of priority requestors.",
            ctx.provider.default_signer_address()
        )));

        // Second order from a different address - should have mcycle limit enforced
        let mut order2 =
            ctx.generate_next_order(OrderParams { order_index: 2, ..Default::default() }).await;
        // Set a different client address
        order2.request.id = RequestId::new(Address::ZERO, 2).into();
        let order2_id = order2.id();

        let locked =
            ctx.picker.price_order_and_update_state(order2, CancellationToken::new()).await;
        assert!(locked);

        // Check logs for the expected message about setting exec limit to max_mcycle_limit
        assert!(logs_contain(&format!(
            "Order {order2_id} prove limit capped by max_mcycle_limit config"
        )));
        assert!(logs_contain(&format!(
            "with limit of {} cycles (~{} mcycles)",
            U256::from(exec_limit) * ONE_MILLION,
            exec_limit
        )));
    }

    #[tokio::test]
    #[traced_test]
    async fn min_mcycle_limit_skips_small_orders() {
        let config = ConfigLock::default();
        {
            let mut cfg = config.load_write().unwrap();
            cfg.market.min_mcycle_price = "0.0000001".into();
            cfg.market.min_mcycle_limit = 50; // Require at least 50 Mcycles
            cfg.market.min_deadline = 0;
        }
        let ctx = PickerTestCtxBuilder::default().with_config(config).build().await;

        let order = ctx
            .generate_loop_order(
                OrderParams {
                    min_price: parse_ether("1").unwrap(),
                    max_price: parse_ether("1").unwrap(),
                    timeout: 3600,
                    lock_timeout: 300,
                    ..Default::default()
                },
                10_000_000,
            )
            .await;

        let order_id = order.id();
        assert!(!ctx.picker.price_order_and_update_state(order, CancellationToken::new()).await);

        assert!(logs_contain(&format!("Order {order_id} skipped due to min_mcycle_limit config")));
        assert!(logs_contain("min_mcycle_limit set to 50 Mcycles in config"));
    }

    #[tokio::test]
    #[traced_test]
    async fn test_deadline_exec_limit_and_peak_prove_khz() {
        let config = ConfigLock::default();
        {
            config.load_write().unwrap().market.min_mcycle_price = "0.0000001".into();
            config.load_write().unwrap().market.peak_prove_khz = Some(1);
            config.load_write().unwrap().market.min_deadline = 10;
        }
        let ctx = PickerTestCtxBuilder::default().with_config(config).build().await;

        let order = ctx
            .generate_next_order(OrderParams {
                min_price: parse_ether("10").unwrap(),
                max_price: parse_ether("10").unwrap(),
                bidding_start: now_timestamp(),
                lock_timeout: 150,
                timeout: 300,
                ..Default::default()
            })
            .await;

        let order_id = order.id();
        let _submit_result =
            ctx.boundless_market.submit_request(&order.request, &ctx.signer(0)).await;

        let locked = ctx.picker.price_order_and_update_state(order, CancellationToken::new()).await;
        assert!(locked);

        let expected_log_pattern = format!("Order {order_id} prove limit capped by deadline");
        assert!(logs_contain(&expected_log_pattern));
    }

    #[tokio::test]
    #[traced_test]
    async fn test_capacity_change() {
        let config = ConfigLock::default();
        {
            let mut cfg = config.load_write().unwrap();
            cfg.market.min_mcycle_price = "0.0000001".into();
            cfg.market.max_concurrent_preflights = 2;
        }
        let mut ctx = PickerTestCtxBuilder::default().with_config(config.clone()).build().await;

        // Start the order picker task
        let picker_task = tokio::spawn(ctx.picker.spawn(Default::default()));

        // Send an initial order to trigger the capacity check
        let order1 =
            ctx.generate_next_order(OrderParams { order_index: 1, ..Default::default() }).await;
        ctx.new_order_tx.send(order1).await.unwrap();

        // Wait for order to be processed
        tokio::time::timeout(Duration::from_secs(10), ctx.priced_orders_rx.recv()).await.unwrap();

        // Sleep to allow for a capacity check change
        tokio::time::sleep(MIN_CAPACITY_CHECK_INTERVAL).await;

        // Decrease capacity
        {
            let mut cfg = config.load_write().unwrap();
            cfg.market.max_concurrent_preflights = 1;
        }

        // Wait a bit more for the interval timer to fire and detect the change
        tokio::time::sleep(MIN_CAPACITY_CHECK_INTERVAL + Duration::from_millis(100)).await;

        // Send another order to trigger capacity check
        let order2 =
            ctx.generate_next_order(OrderParams { order_index: 2, ..Default::default() }).await;
        ctx.new_order_tx.send(order2).await.unwrap();

        // Wait for an order to be processed before updating capacity
        tokio::time::timeout(Duration::from_secs(10), ctx.priced_orders_rx.recv()).await.unwrap();

        // Check logs for capacity changes
        assert!(logs_contain("Pricing capacity changed from 2 to 1"));

        picker_task.abort();
    }

    #[tokio::test]
    #[traced_test]
    async fn test_lock_expired_exec_limit_precision_loss() {
        let config = ConfigLock::default();
        let min_deadline = {
            let cfg = config.lock_all().unwrap();
            cfg.market.min_deadline
        };
        {
            config.load_write().unwrap().market.min_mcycle_price_collateral_token = "1".into();
        }
        let ctx = PickerTestCtxBuilder::default()
            .with_config(config.clone())
            .with_collateral_token_decimals(6)
            .build()
            .await;

        let timeout = (min_deadline + 200) as u32;

        let mut order = ctx
            .generate_next_order(OrderParams {
                lock_collateral: U256::from(1),
                fulfillment_type: FulfillmentType::FulfillAfterLockExpire,
                bidding_start: now_timestamp() - 100,
                lock_timeout: 10,
                timeout,
                ..Default::default()
            })
            .await;

        let collateral_reward = order.request.offer.collateral_reward_if_locked_and_not_fulfilled();
        assert_eq!(collateral_reward, U256::from(0));

        let locked = ctx.picker.price_order(&mut order).await;
        assert!(
            matches!(locked, Ok(OrderPricingOutcome::Skip { reason }) if reason.contains("exec limit is too low"))
        );

        let mut order2 = ctx
            .generate_next_order(OrderParams {
                order_index: 2,
                lock_collateral: U256::from(40),
                fulfillment_type: FulfillmentType::FulfillAfterLockExpire,
                bidding_start: now_timestamp() - 100,
                lock_timeout: 10,
                timeout,
                ..Default::default()
            })
            .await;

        let order2_id = order2.id();
        let collateral_reward2 =
            order2.request.offer.collateral_reward_if_locked_and_not_fulfilled();
        assert_eq!(collateral_reward2, U256::from(20));

        let locked = ctx.picker.price_order(&mut order2).await;
        assert!(matches!(locked, Ok(OrderPricingOutcome::Skip { .. })));

        // collateral token denom offsets the mcycle multiplier, so for 1collateral/mcycle, this will be 10
        assert!(logs_contain(&format!(
            "Starting preflight execution of {order2_id} with limit of 20 cycles"
        )));
        assert!(logs_contain(&format!(
            "Skipping order {order2_id} due to intentional execution limit of"
        )));
    }

    #[tokio::test]
    #[traced_test]
    async fn test_order_is_locked_check() -> Result<()> {
        let ctx = PickerTestCtxBuilder::default().build().await;

        let mut order = ctx.generate_next_order(Default::default()).await;

        ctx.db
            .set_request_locked(
                U256::from(order.request.id),
                &ctx.provider.default_signer_address().to_string(),
                1000,
            )
            .await?;

        assert!(ctx.db.is_request_locked(U256::from(order.request.id)).await?);

        let pricing_outcome = ctx.picker.price_order(&mut order).await?;
        assert!(
            matches!(pricing_outcome, OrderPricingOutcome::Skip { reason } if reason.contains("already locked"))
        );

        Ok(())
    }

    #[tokio::test]
    #[traced_test]
    async fn test_duplicate_order_cache() -> Result<()> {
        let mut ctx = PickerTestCtxBuilder::default().build().await;

        let order1 = ctx.generate_next_order(Default::default()).await;
        let order_id = order1.id();

        // Duplicate order
        let order2 = Box::new(OrderRequest {
            request: order1.request.clone(),
            client_sig: order1.client_sig.clone(),
            fulfillment_type: order1.fulfillment_type,
            boundless_market_address: order1.boundless_market_address,
            chain_id: order1.chain_id,
            image_id: order1.image_id.clone(),
            input_id: order1.input_id.clone(),
            total_cycles: order1.total_cycles,
            target_timestamp: order1.target_timestamp,
            expire_timestamp: order1.expire_timestamp,
            cached_id: Default::default(),
        });

        assert_eq!(order1.id(), order2.id(), "Both orders should have the same ID");

        tokio::spawn(ctx.picker.spawn(CancellationToken::new()));

        ctx.new_order_tx.send(order1).await?;
        ctx.new_order_tx.send(order2).await?;

        let first_processed =
            tokio::time::timeout(Duration::from_secs(10), ctx.priced_orders_rx.recv())
                .await?
                .unwrap();

        assert_eq!(first_processed.id(), order_id, "First order should be processed");

        let second_result =
            tokio::time::timeout(Duration::from_secs(2), ctx.priced_orders_rx.recv()).await;

        assert!(second_result.is_err(), "Second order should be deduplicated and not processed");

        assert!(logs_contain(&format!("Skipping order {order_id} - already being processed")));

        Ok(())
    }

    #[tokio::test]
    #[traced_test]
    async fn test_order_is_fulfilled_check() -> Result<()> {
        let ctx = PickerTestCtxBuilder::default().build().await;

        let mut order = ctx
            .generate_next_order(OrderParams {
                fulfillment_type: FulfillmentType::FulfillAfterLockExpire,
                ..Default::default()
            })
            .await;

        ctx.db.set_request_fulfilled(U256::from(order.request.id), 1000).await?;

        assert!(ctx.db.is_request_fulfilled(U256::from(order.request.id)).await?);

        let pricing_outcome = ctx.picker.price_order(&mut order).await?;
        assert!(
            matches!(pricing_outcome, OrderPricingOutcome::Skip { reason } if reason.contains("already fulfilled"))
        );

        Ok(())
    }

    #[tokio::test]
    #[traced_test]
    async fn test_active_tasks_logging() {
        let config = ConfigLock::default();
        {
            config.load_write().unwrap().market.min_mcycle_price = "0.0000001".into();
        }
        let mut ctx = PickerTestCtxBuilder::default().with_config(config).build().await;

        // Start the order picker task
        let picker_task = tokio::spawn(ctx.picker.spawn(Default::default()));

        // Send an order to trigger the logging
        let order1 =
            ctx.generate_next_order(OrderParams { order_index: 1, ..Default::default() }).await;
        let order1_id = order1.id();
        ctx.new_order_tx.send(order1).await.unwrap();

        // Wait for the order to be processed and check for the "Added" log
        tokio::time::timeout(MIN_CAPACITY_CHECK_INTERVAL * 2, ctx.priced_orders_rx.recv())
            .await
            .unwrap();

        // Check that we logged the task being added (or that it was completed before the interval)
        assert!(logs_contain("Current pricing tasks: [") || logs_contain("Priced task for order"));
        assert!(logs_contain(&order1_id));

        // Send another order to see the task being removed and a new one added
        let order2 =
            ctx.generate_next_order(OrderParams { order_index: 2, ..Default::default() }).await;
        let order2_id = order2.id();
        ctx.new_order_tx.send(order2).await.unwrap();

        // Wait for the second order to be processed
        tokio::time::timeout(Duration::from_secs(5), ctx.priced_orders_rx.recv()).await.unwrap();

        // Check that we logged the task completion
        assert!(logs_contain(&format!("Priced task for order {order1_id} (request")));

        // The order2 should be shown as in progress when order1 completes
        assert!(logs_contain(&order2_id));

        picker_task.abort();
    }

    #[tokio::test]
    async fn test_handle_lock_event() {
        let ctx = PickerTestCtxBuilder::default().build().await;
        let mut active_tasks: BTreeMap<U256, BTreeMap<String, CancellationToken>> = BTreeMap::new();
        let mut pending_orders: Vec<Box<OrderRequest>> = Vec::new();

        let lock_and_fulfill_order = ctx
            .generate_next_order(OrderParams {
                order_index: 123,
                fulfillment_type: FulfillmentType::LockAndFulfill,
                ..Default::default()
            })
            .await;

        let fulfill_after_expire_order = ctx
            .generate_next_order(OrderParams {
                order_index: 123,
                fulfillment_type: FulfillmentType::FulfillAfterLockExpire,
                ..Default::default()
            })
            .await;

        let request_id = U256::from(lock_and_fulfill_order.request.id);

        let lock_and_fulfill_token = CancellationToken::new();
        let fulfill_after_expire_token = CancellationToken::new();

        // Add active tasks using actual order IDs
        let mut order_tasks = BTreeMap::new();
        order_tasks.insert(lock_and_fulfill_order.id(), lock_and_fulfill_token.clone());
        order_tasks.insert(fulfill_after_expire_order.id(), fulfill_after_expire_token.clone());
        active_tasks.insert(request_id, order_tasks);

        pending_orders.push(lock_and_fulfill_order);
        pending_orders.push(fulfill_after_expire_order);

        handle_lock_event(request_id, &mut active_tasks, &mut pending_orders);

        assert!(lock_and_fulfill_token.is_cancelled(), "LockAndFulfill task should be cancelled");
        assert!(
            !fulfill_after_expire_token.is_cancelled(),
            "FulfillAfterLockExpire task should NOT be cancelled"
        );

        assert!(active_tasks.contains_key(&request_id));
        let remaining_tasks = active_tasks.get(&request_id).unwrap();
        assert_eq!(remaining_tasks.len(), 1);
        let remaining_order_id = remaining_tasks.keys().next().unwrap();
        assert!(remaining_order_id.contains("FulfillAfterLockExpire"));

        assert_eq!(pending_orders.len(), 1);
        assert_eq!(pending_orders[0].fulfillment_type, FulfillmentType::FulfillAfterLockExpire);
    }

    #[tokio::test]
    async fn test_handle_fulfill_event() {
        // Create test context and orders
        let ctx = PickerTestCtxBuilder::default().build().await;
        let mut active_tasks: BTreeMap<U256, BTreeMap<String, CancellationToken>> = BTreeMap::new();
        let mut pending_orders: Vec<Box<OrderRequest>> = Vec::new();

        let lock_and_fulfill_order = ctx
            .generate_next_order(OrderParams {
                order_index: 456,
                fulfillment_type: FulfillmentType::LockAndFulfill,
                ..Default::default()
            })
            .await;

        let fulfill_after_expire_order = ctx
            .generate_next_order(OrderParams {
                order_index: 456,
                fulfillment_type: FulfillmentType::FulfillAfterLockExpire,
                ..Default::default()
            })
            .await;

        let request_id = U256::from(lock_and_fulfill_order.request.id);

        let token1 = CancellationToken::new();
        let token2 = CancellationToken::new();

        let mut order_tasks = BTreeMap::new();
        order_tasks.insert(lock_and_fulfill_order.id(), token1.clone());
        order_tasks.insert(fulfill_after_expire_order.id(), token2.clone());
        active_tasks.insert(request_id, order_tasks);

        pending_orders.push(lock_and_fulfill_order);
        pending_orders.push(fulfill_after_expire_order);

        handle_fulfill_event(request_id, &mut active_tasks, &mut pending_orders);

        assert!(token1.is_cancelled(), "All tasks should be cancelled");
        assert!(token2.is_cancelled(), "All tasks should be cancelled");

        assert!(!active_tasks.contains_key(&request_id));

        assert_eq!(pending_orders.len(), 0, "All pending orders should be removed");
    }

    // Mock prover that tracks preflight calls
    struct MockPreflightTracker {
        preflight_calls: Arc<std::sync::Mutex<Vec<(String, String)>>>,
        default_prover: Arc<DefaultProver>,
    }

    impl MockPreflightTracker {
        fn new() -> Self {
            Self {
                preflight_calls: Arc::new(std::sync::Mutex::new(Vec::new())),
                default_prover: Arc::new(DefaultProver::new()),
            }
        }

        fn get_preflight_calls(&self) -> Vec<(String, String)> {
            self.preflight_calls.lock().unwrap().clone()
        }
    }

    #[async_trait]
    impl Prover for MockPreflightTracker {
        async fn upload_image(&self, image_id: &str, image: Vec<u8>) -> Result<(), ProverError> {
            self.default_prover.upload_image(image_id, image).await
        }

        async fn upload_input(&self, input: Vec<u8>) -> Result<String, ProverError> {
            self.default_prover.upload_input(input).await
        }

        async fn preflight(
            &self,
            image_id: &str,
            input_id: &str,
            assumptions: Vec<String>,
            executor_limit: Option<u64>,
            order_id: &str,
        ) -> Result<ProofResult, ProverError> {
            // Track the preflight call
            self.preflight_calls.lock().unwrap().push((image_id.to_string(), input_id.to_string()));

            // Call the default prover
            self.default_prover
                .preflight(image_id, input_id, assumptions, executor_limit, order_id)
                .await
        }

        async fn has_image(&self, image_id: &str) -> Result<bool, ProverError> {
            self.default_prover.has_image(image_id).await
        }

        async fn prove_stark(
            &self,
            image_id: &str,
            input_id: &str,
            assumptions: Vec<String>,
        ) -> Result<String, ProverError> {
            self.default_prover.prove_stark(image_id, input_id, assumptions).await
        }

        async fn wait_for_stark(&self, proof_id: &str) -> Result<ProofResult, ProverError> {
            self.default_prover.wait_for_stark(proof_id).await
        }

        async fn cancel_stark(&self, proof_id: &str) -> Result<(), ProverError> {
            self.default_prover.cancel_stark(proof_id).await
        }

        async fn get_receipt(&self, proof_id: &str) -> Result<Option<Receipt>, ProverError> {
            self.default_prover.get_receipt(proof_id).await
        }

        async fn get_preflight_journal(
            &self,
            proof_id: &str,
        ) -> Result<Option<Vec<u8>>, ProverError> {
            self.default_prover.get_preflight_journal(proof_id).await
        }

        async fn get_journal(&self, proof_id: &str) -> Result<Option<Vec<u8>>, ProverError> {
            self.default_prover.get_journal(proof_id).await
        }

        async fn compress(&self, proof_id: &str) -> Result<String, ProverError> {
            self.default_prover.compress(proof_id).await
        }

        async fn get_compressed_receipt(
            &self,
            proof_id: &str,
        ) -> Result<Option<Vec<u8>>, ProverError> {
            self.default_prover.get_compressed_receipt(proof_id).await
        }
        async fn compress_blake3_groth16(&self, proof_id: &str) -> Result<String, ProverError> {
            self.default_prover.compress_blake3_groth16(proof_id).await
        }

        async fn get_blake3_groth16_receipt(
            &self,
            proof_id: &str,
        ) -> Result<Option<Vec<u8>>, ProverError> {
            self.default_prover.get_blake3_groth16_receipt(proof_id).await
        }
    }

    #[tokio::test]
    #[traced_test]
    async fn test_preflight_cache_behavior() -> Result<()> {
        let mock_prover = Arc::new(MockPreflightTracker::new());

        let image_id = Digest::from(ECHO_ID).to_string();
        mock_prover.upload_image(&image_id, ECHO_ELF.to_vec()).await.unwrap();

        let ctx = PickerTestCtxBuilder::default().with_prover(mock_prover.clone()).build().await;

        let mut order1 =
            ctx.generate_next_order(OrderParams { order_index: 100, ..Default::default() }).await;

        let mut order2 =
            ctx.generate_next_order(OrderParams { order_index: 200, ..Default::default() }).await;

        let mut order3 = ctx
            .generate_next_order(OrderParams {
                order_index: 100,
                fulfillment_type: FulfillmentType::FulfillAfterLockExpire,
                ..Default::default()
            })
            .await;

        assert_eq!(
            order1.request.id, order3.request.id,
            "Order1 and Order3 should have same request ID"
        );
        assert_ne!(
            order1.request.id, order2.request.id,
            "Order1 and Order2 should have different request IDs"
        );

        // Process order1 and order2 concurrently to test cache atomicity
        let (pricing1, pricing2) =
            tokio::join!(ctx.picker.price_order(&mut order1), ctx.picker.price_order(&mut order2));

        assert!(pricing1.is_ok(), "Order1 pricing should succeed");
        assert!(pricing2.is_ok(), "Order2 pricing should succeed");

        // Process order3 (should use cache)
        let pricing3 = ctx.picker.price_order(&mut order3).await;
        assert!(pricing3.is_ok(), "Order3 pricing should succeed");

        // Check preflight calls - should only be called once since all orders are identical
        let preflight_calls = mock_prover.get_preflight_calls();

        // Since ALL orders have the same image_url and input data, they should share the same cache entry
        assert_eq!(
            preflight_calls.len(),
            1,
            "Should have exactly 1 preflight call since all orders are identical.",
        );

        Ok(())
    }

    #[tokio::test]
    #[traced_test]
    async fn test_smaller_cycle_limit_cache() -> Result<()> {
        let mock_prover = Arc::new(MockPreflightTracker::new());
        let image_id = Digest::from(LOOP_ID).to_string();
        mock_prover.upload_image(&image_id, LOOP_ELF.to_vec()).await.unwrap();

        // Create context with very low mcycle price and set peak_prove_khz to create different deadline caps
        let config = ConfigLock::default();
        {
            config.load_write().unwrap().market.min_mcycle_price = "0.0000001".into();
            config.load_write().unwrap().market.peak_prove_khz = Some(1000); // Set peak_prove_khz to create deadline caps
            config.load_write().unwrap().market.min_deadline = 0; // Remove min_deadline interference
        }
        let ctx = PickerTestCtxBuilder::default()
            .with_prover(mock_prover.clone())
            .with_config(config)
            .build()
            .await;

        // Create two orders with same program+input but very different exec limits due to different timeouts:
        // Order 1: Very short timeout = very low deadline cap (should hit session limit exceeded)
        // We'll set the loop to consume 50M cycles, which exceeds the 20M cycle cap from short timeout
        let mut low_timeout_order = ctx
            .generate_loop_order(
                OrderParams {
                    order_index: 1,
                    min_price: parse_ether("100.0").unwrap(), // High price but will be capped by very short timeout
                    max_price: parse_ether("100.0").unwrap(),
                    timeout: 30, // Very short timeout = very low deadline cap (30s * 1000khz = 30M cycles)
                    lock_timeout: 2, // Also set short lock_timeout
                    ..Default::default()
                },
                5_000_000,
            ) // 5M cycles - should exceed the 20M cycle limit
            .await;

        // Order 2: Long timeout = high deadline cap (should succeed and NOT reuse low-limit cache)
        // Same cycle count but much higher exec limit due to longer timeout
        let mut high_timeout_order = ctx
            .generate_loop_order(
                OrderParams {
                    order_index: 2,
                    min_price: parse_ether("100.0").unwrap(), // Same high price but much longer timeout
                    max_price: parse_ether("100.0").unwrap(),
                    timeout: 3600, // Much longer timeout = high deadline cap (3600s * 1000khz = 3.6B cycles)
                    lock_timeout: 3000, // Also set long lock_timeout
                    ..Default::default()
                },
                5_000_000,
            ) // Same 5M cycles - should be under the 3B cycle limit
            .await;

        // Process short timeout order first - this should hit session limit and cache the Skip result
        let result1 = ctx.picker.price_order(&mut low_timeout_order).await;
        assert!(matches!(result1, Ok(OrderPricingOutcome::Skip { .. })));

        // Process long timeout order second - this should NOT reuse the low-limit cached result
        // It should succeed with its own higher exec limit via a new preflight call
        let result2 = ctx.picker.price_order(&mut high_timeout_order).await;
        assert!(matches!(result2, Ok(OrderPricingOutcome::Lock { .. })));

        // We expect 2 preflight calls since the orders have different deadline-based exec limits
        let preflight_calls = mock_prover.get_preflight_calls();
        assert_eq!(
            preflight_calls.len(),
            1,
            "Should have exactly 1 preflight call due to preflight result caching that avoids redundant execution.",
        );

        Ok(())
    }

    #[tokio::test]
    #[traced_test]
    async fn test_concurrent_preflights_with_cancellation() -> Result<()> {
        let mock_prover = Arc::new(MockPreflightTracker::new());
        let image_id = Digest::from(LOOP_ID).to_string();
        mock_prover.upload_image(&image_id, LOOP_ELF.to_vec()).await.unwrap();

        let config = ConfigLock::default();
        {
            config.load_write().unwrap().market.min_mcycle_price = "0.0000001".into();
        }
        let ctx = PickerTestCtxBuilder::default()
            .with_prover(mock_prover.clone())
            .with_config(config)
            .build()
            .await;

        // Create two orders with same program+input for same cache key
        let order_a = ctx
            .generate_loop_order(
                OrderParams {
                    order_index: 1,
                    min_price: parse_ether("100.0").unwrap(),
                    max_price: parse_ether("100.0").unwrap(),
                    timeout: 3600,
                    lock_timeout: 3000,
                    ..Default::default()
                },
                5_000_000,
            )
            .await;

        let order_b = ctx
            .generate_loop_order(
                OrderParams {
                    order_index: 2,
                    min_price: parse_ether("100.0").unwrap(),
                    max_price: parse_ether("100.0").unwrap(),
                    timeout: 3600,
                    lock_timeout: 3000,
                    ..Default::default()
                },
                5_000_000,
            )
            .await;

        // Create cancellation tokens
        let cancel_token_a = CancellationToken::new();
        let cancel_token_b = CancellationToken::new();

        // Save order IDs before moving into tasks
        let order_a_id = order_a.id();
        let _order_b_id = order_b.id();

        // Start both preflights concurrently with a slight stagger
        let cancel_a_clone = cancel_token_a.clone();
        let picker_a = ctx.picker.clone();
        let task_a = tokio::spawn(async move {
            picker_a.price_order_and_update_state(order_a, cancel_token_a).await
        });

        // Small delay to ensure task A starts first
        tokio::time::sleep(Duration::from_millis(10)).await;

        let picker_b = ctx.picker.clone();
        let task_b = tokio::spawn(async move {
            picker_b.price_order_and_update_state(order_b, cancel_token_b).await
        });

        // Wait for task A to start its preflight before cancelling
        loop {
            tokio::time::sleep(Duration::from_millis(10)).await;
            if logs_contain(&format!("Starting preflight of {order_a_id}")) {
                // Sleep to wait for B to wait on this preflight
                tokio::time::sleep(Duration::from_millis(20)).await;
                break;
            }
        }

        // Cancel task A now that we know it has started preflight
        cancel_a_clone.cancel();

        // Wait for both tasks to complete
        let result_a = task_a.await.unwrap();
        let result_b = task_b.await.unwrap();

        // Task A should have been cancelled and returned false
        assert!(!result_a, "Task A should have been cancelled");

        // Task B should have completed successfully
        assert!(result_b, "Task B should have completed successfully");

        // Check that the cancellation was logged
        assert!(logs_contain("Order pricing cancelled during pricing for order"));

        // Verify that preflights are cached to avoid redundant execution
        // Both orders should reuse the same preflight result due to caching
        let preflight_calls = mock_prover.get_preflight_calls();
        assert_eq!(preflight_calls.len(), 1, "Should have exactly 1 preflight call due to caching");

        Ok(())
    }

    // Unit tests for calculate_exec_limits function

    /// Helper to parse collateral token amounts (6 decimals) instead of ETH (18 decimals)
    fn parse_collateral_tokens(amount: &str) -> U256 {
        parse_units(amount, 6).unwrap().into()
    }

    #[tokio::test]
    async fn test_calculate_exec_limits_eth_higher_than_collateral() {
        let market_config = MarketConf {
            min_mcycle_price: "0.001".to_string(), // 0.01 ETH per mcycle
            min_mcycle_price_collateral_token: "10".to_string(), // 10 collateral tokens per mcycle
            max_mcycle_limit: 8000,
            peak_prove_khz: None,
            priority_requestor_addresses: None,
            ..Default::default()
        };

        let ctx = PickerTestCtxBuilder::default()
            .with_config({
                let config = ConfigLock::default();
                config.load_write().unwrap().market = market_config;
                config
            })
            .build()
            .await;

        let gas_cost = parse_ether("0.001").unwrap(); // 0.001 ETH gas cost

        let order = ctx
            .generate_next_order(OrderParams {
                fulfillment_type: FulfillmentType::LockAndFulfill,
                max_price: parse_ether("0.05").unwrap(), // 0.05 ETH max price
                lock_collateral: parse_collateral_tokens("100"), // 100 collateral tokens
                lock_timeout: 900,                       // lock timeout
                timeout: 1200,                           // order timeout
                ..Default::default()
            })
            .await;

        // For lock and fulfill, if the exec limit based on ETH is higher than the exec
        // limit based on collateral, we should use the ETH limit.
        let (preflight_limit, prove_limit, _reason) =
            ctx.picker.calculate_exec_limits(&order, gas_cost).unwrap();

        // ETH based: (0.05 ETH - 0.001 ETH) * 1M / 0.001 ETH/mcycle = 49M cycles
        // collateral based: (100 collateral tokens - 20% collateral burn) * 1M / 10 collateral_tokens/mcycle = 8M cycles
        // ETH-based is higher, so both should use ETH-based limit

        assert_eq!(preflight_limit, 49_000_000u64);
        assert_eq!(prove_limit, 49_000_000u64);
    }

    #[tokio::test]
    async fn test_calculate_exec_limits_collateral_higher_than_eth_exposes_bug() {
        let market_config = MarketConf {
            min_mcycle_price: "0.1".to_string(), // 0.1 ETH per mcycle (expensive)
            min_mcycle_price_collateral_token: "1".to_string(), // 1 collateral token per mcycle (cheaper)
            max_mcycle_limit: 8000,
            peak_prove_khz: None,
            priority_requestor_addresses: None,
            ..Default::default()
        };

        let ctx = PickerTestCtxBuilder::default()
            .with_config({
                let config = ConfigLock::default();
                config.load_write().unwrap().market = market_config;
                config
            })
            .build()
            .await;

        let gas_cost = parse_ether("0.001").unwrap();

        let order = ctx
            .generate_next_order(OrderParams {
                fulfillment_type: FulfillmentType::LockAndFulfill,
                max_price: parse_ether("0.05").unwrap(), // 0.05 ETH max price
                lock_collateral: parse_collateral_tokens("1000"), // 1000 collateral tokens
                lock_timeout: 900,
                timeout: 1200,
                ..Default::default()
            })
            .await;

        let (preflight_limit, prove_limit, _reason) =
            ctx.picker.calculate_exec_limits(&order, gas_cost).unwrap();

        // ETH based: (0.05 ETH - 0.001 ETH) * 1M / 0.1 ETH/mcycle = 490k cycles
        // collateral based: (1000 collateral tokens - 20% burn) * 1M / 1 collateral_token/mcycle = 800M cycles
        // collateral-based is higher, demonstrating the bug where preflight != prove limits

        let collateral_reward_tokens = order
            .request
            .offer
            .collateral_reward_if_locked_and_not_fulfilled()
            .div_ceil(U256::from(1_000_000));
        let collateral_based_limit: u64 = collateral_reward_tokens
            .saturating_mul(U256::from(1_000_000))
            .div_ceil(U256::from(1))
            .try_into()
            .unwrap();
        assert_eq!(preflight_limit, collateral_based_limit);
        assert_eq!(prove_limit, 490_000u64);
    }

    #[traced_test]
    #[tokio::test]
    async fn test_calculate_exec_limits_fulfill_after_expire_collateral_only() {
        let market_config = MarketConf {
            min_mcycle_price: "0.0134".to_string(), // Won't be used for FulfillAfterLockExpire
            min_mcycle_price_collateral_token: "0.1".to_string(), // 0.1 collateral per mcycle
            max_mcycle_limit: 8000,
            peak_prove_khz: None,
            priority_requestor_addresses: None,
            ..Default::default()
        };

        let ctx = PickerTestCtxBuilder::default()
            .with_config({
                let config = ConfigLock::default();
                config.load_write().unwrap().market = market_config;
                config
            })
            .build()
            .await;

        let gas_cost = parse_ether("0.001").unwrap();

        let order = ctx
            .generate_next_order(OrderParams {
                fulfillment_type: FulfillmentType::FulfillAfterLockExpire,
                max_price: parse_ether("1.0").unwrap(), // Won't be used
                lock_collateral: parse_collateral_tokens("100"), // 100 collateral tokens
                lock_timeout: 900,
                timeout: 1200,
                ..Default::default()
            })
            .await;

        let (preflight_limit, prove_limit, _reason) =
            ctx.picker.calculate_exec_limits(&order, gas_cost).unwrap();

        // Should only use collateral-based pricing for FulfillAfterLockExpire
        // collateral based: (100 collateral tokens - 20% burn) / 0.1 collateral tokens per mcycle = 80M cycles
        let collateral_reward_tokens =
            order.request.offer.collateral_reward_if_locked_and_not_fulfilled();
        tracing::info!("collateral reward tokens: {collateral_reward_tokens}");
        let collateral_based_limit: u64 = collateral_reward_tokens
            .saturating_mul(U256::from(1_000_000))
            .div_ceil(parse_collateral_tokens("0.1"))
            .try_into()
            .unwrap();

        assert_eq!(preflight_limit, collateral_based_limit);
        assert_eq!(prove_limit, collateral_based_limit);
    }

    #[tokio::test]
    async fn test_calculate_exec_limits_max_mcycle_cap() {
        let market_config = MarketConf {
            min_mcycle_price: "0.01".to_string(),
            min_mcycle_price_collateral_token: "0.1".to_string(),
            max_mcycle_limit: 20, // 20 mcycle limit
            peak_prove_khz: None,
            priority_requestor_addresses: None,
            ..Default::default()
        };

        let ctx = PickerTestCtxBuilder::default()
            .with_config({
                let config = ConfigLock::default();
                config.load_write().unwrap().market = market_config;
                config
            })
            .build()
            .await;

        let gas_cost = parse_ether("0.001").unwrap();

        let order = ctx
            .generate_next_order(OrderParams {
                fulfillment_type: FulfillmentType::LockAndFulfill,
                max_price: parse_ether("10.0").unwrap(), // Very high price
                lock_collateral: parse_collateral_tokens("1000.0"), // Very high collateral
                lock_timeout: 900,
                timeout: 1200,
                ..Default::default()
            })
            .await;

        let (preflight_limit, prove_limit, _reason) =
            ctx.picker.calculate_exec_limits(&order, gas_cost).unwrap();

        // Should be capped at 20M cycles regardless of high prices
        let expected_cycles = 20_000_000u64;

        assert_eq!(preflight_limit, expected_cycles);
        assert_eq!(prove_limit, expected_cycles);
    }

    #[tokio::test]
    async fn test_calculate_exec_limits_priority_requestor_unlimited() {
        let priority_address = address!("1234567890123456789012345678901234567890");
        let market_config = MarketConf {
            min_mcycle_price: "0.01".to_string(),
            min_mcycle_price_collateral_token: "0.1".to_string(),
            max_mcycle_limit: 5, // Low limit normally
            peak_prove_khz: None,
            priority_requestor_addresses: Some(vec![priority_address]),
            ..Default::default()
        };

        let ctx = PickerTestCtxBuilder::default()
            .with_config({
                let config = ConfigLock::default();
                config.load_write().unwrap().market = market_config;
                config
            })
            .build()
            .await;

        let gas_cost = parse_ether("0.001").unwrap();

        let mut order = ctx
            .generate_next_order(OrderParams {
                fulfillment_type: FulfillmentType::LockAndFulfill,
                max_price: parse_ether("1.0").unwrap(),
                lock_collateral: parse_collateral_tokens("100.0"),
                lock_timeout: 900,
                timeout: 1200,
                ..Default::default()
            })
            .await;

        // Set the client address to the priority address
        order.request.id = RequestId::new(priority_address, 1).into();

        let (preflight_limit, prove_limit, _reason) =
            ctx.picker.calculate_exec_limits(&order, gas_cost).unwrap();

        // Priority requestors ignore max_mcycle_limit but use different calculations for preflight vs prove
        // For LockAndFulfill orders: preflight uses higher limit (collateral), prove uses ETH-based
        assert_eq!(preflight_limit, 500_000_000u64); // collateral-based calculation
        assert_eq!(prove_limit, 99_900_000u64); // ETH-based calculation
    }

    #[tokio::test]
    async fn test_calculate_exec_limits_timing_constraints() {
        let market_config = MarketConf {
            min_mcycle_price: "0.01".to_string(),
            min_mcycle_price_collateral_token: "0.1".to_string(),
            max_mcycle_limit: 8000,
            peak_prove_khz: Some(1000), // 1M cycles per second
            priority_requestor_addresses: None,
            ..Default::default()
        };

        let ctx = PickerTestCtxBuilder::default()
            .with_config({
                let config = ConfigLock::default();
                config.load_write().unwrap().market = market_config;
                config
            })
            .build()
            .await;

        let gas_cost = parse_ether("0.001").unwrap();

        let order = ctx
            .generate_next_order(OrderParams {
                fulfillment_type: FulfillmentType::LockAndFulfill,
                max_price: parse_ether("10.0").unwrap(), // High price that would normally allow many cycles
                lock_collateral: parse_collateral_tokens("100.0"), // High collateral
                lock_timeout: 60,                        // 60 second lock timeout
                timeout: 120,                            // 120 second order timeout
                ..Default::default()
            })
            .await;

        let (preflight_limit, prove_limit, _reason) =
            ctx.picker.calculate_exec_limits(&order, gas_cost).unwrap();

        // Should be limited by timing constraints
        // Prove window: 60 seconds -> 60M cycles max
        // Both should be capped at 60M cycles despite high prices
        let expected_cycles = 60_000_000u64;

        assert_eq!(preflight_limit, expected_cycles);
        assert_eq!(prove_limit, expected_cycles);
    }

    #[tokio::test]
    async fn test_calculate_exec_limits_zero_collateral_price_unlimited() {
        let market_config = MarketConf {
            min_mcycle_price: "0.01".to_string(),
            min_mcycle_price_collateral_token: "0".to_string(), // Zero collateral price
            max_mcycle_limit: u64::MAX,
            peak_prove_khz: None,
            priority_requestor_addresses: None,
            ..Default::default()
        };

        let ctx = PickerTestCtxBuilder::default()
            .with_config({
                let config = ConfigLock::default();
                config.load_write().unwrap().market = market_config;
                config
            })
            .build()
            .await;

        let gas_cost = parse_ether("0.001").unwrap();

        let order = ctx
            .generate_next_order(OrderParams {
                fulfillment_type: FulfillmentType::FulfillAfterLockExpire,
                max_price: parse_ether("1.0").unwrap(),
                lock_collateral: parse_collateral_tokens("10.0"),
                lock_timeout: 900,
                timeout: 1200,
                ..Default::default()
            })
            .await;

        let (preflight_limit, prove_limit, _reason) =
            ctx.picker.calculate_exec_limits(&order, gas_cost).unwrap();

        // Should be unlimited (u64::MAX) when collateral price is zero
        assert_eq!(preflight_limit, u64::MAX);
        assert_eq!(prove_limit, u64::MAX);
    }

    #[tokio::test]
    async fn test_calculate_exec_limits_very_short_deadline() {
        let market_config = MarketConf {
            min_mcycle_price: "0.01".to_string(),
            min_mcycle_price_collateral_token: "0.1".to_string(),
            max_mcycle_limit: 8000,
            peak_prove_khz: Some(1000), // 1M cycles per second
            priority_requestor_addresses: None,
            ..Default::default()
        };

        let ctx = PickerTestCtxBuilder::default()
            .with_config({
                let config = ConfigLock::default();
                config.load_write().unwrap().market = market_config;
                config
            })
            .build()
            .await;

        let gas_cost = parse_ether("0.001").unwrap();

        let order = ctx
            .generate_next_order(OrderParams {
                fulfillment_type: FulfillmentType::LockAndFulfill,
                max_price: parse_ether("1.0").unwrap(), // High price
                lock_collateral: parse_collateral_tokens("10.0"), // High collateral
                lock_timeout: 1,                        // 1 second lock timeout (very short!)
                timeout: 2,                             // 2 second order timeout
                ..Default::default()
            })
            .await;

        let (preflight_limit, prove_limit, _reason) =
            ctx.picker.calculate_exec_limits(&order, gas_cost).unwrap();

        // Should be limited by very short deadline: 1 second = 1M cycles
        let expected_cycles = 1_000_000u64;

        assert_eq!(preflight_limit, expected_cycles);
        assert_eq!(prove_limit, expected_cycles);
    }

    #[tokio::test]
    async fn test_calculate_exec_limits_zero_mcycle_price_unlimited() {
        let market_config = MarketConf {
            min_mcycle_price: "0".to_string(), // Zero ETH price
            min_mcycle_price_collateral_token: "0.1".to_string(),
            max_mcycle_limit: u64::MAX,
            peak_prove_khz: None,
            priority_requestor_addresses: None,
            ..Default::default()
        };

        let ctx = PickerTestCtxBuilder::default()
            .with_config({
                let config = ConfigLock::default();
                config.load_write().unwrap().market = market_config;
                config
            })
            .build()
            .await;

        let gas_cost = parse_ether("0.001").unwrap();

        let order = ctx
            .generate_next_order(OrderParams {
                fulfillment_type: FulfillmentType::LockAndFulfill,
                max_price: parse_ether("1.0").unwrap(),
                lock_collateral: parse_collateral_tokens("10.0"),
                lock_timeout: 60,
                timeout: 120,
                ..Default::default()
            })
            .await;

        let (preflight_limit, prove_limit, _reason) =
            ctx.picker.calculate_exec_limits(&order, gas_cost).unwrap();

        // Should be unlimited (u64::MAX) when ETH mcycle_price is zero
        assert_eq!(preflight_limit, u64::MAX);
        assert_eq!(prove_limit, u64::MAX);
    }

    #[tokio::test]
    #[traced_test]
    async fn test_zero_mcycle_price_order_processing() {
        let config = ConfigLock::default();
        {
            config.load_write().unwrap().market.min_mcycle_price = "0".into();
        }
        let mut ctx = PickerTestCtxBuilder::default().with_config(config).build().await;

        let order = ctx.generate_next_order(Default::default()).await;

        let _request_id =
            ctx.boundless_market.submit_request(&order.request, &ctx.signer(0)).await.unwrap();

        let locked = ctx.picker.price_order_and_update_state(order, CancellationToken::new()).await;
        assert!(locked);

        let priced_order = ctx.priced_orders_rx.try_recv().unwrap();
        assert_eq!(priced_order.target_timestamp, Some(0));
    }

    #[tokio::test]
    #[traced_test]
    async fn test_zero_collateral_price_order_processing() {
        let config = ConfigLock::default();
        {
            config.load_write().unwrap().market.min_mcycle_price_collateral_token = "0".into();
        }
        let mut ctx = PickerTestCtxBuilder::default().with_config(config).build().await;

        let order = ctx
            .generate_next_order(OrderParams {
                fulfillment_type: FulfillmentType::FulfillAfterLockExpire,
                ..Default::default()
            })
            .await;

        let _request_id =
            ctx.boundless_market.submit_request(&order.request, &ctx.signer(0)).await.unwrap();

        let locked = ctx.picker.price_order_and_update_state(order, CancellationToken::new()).await;
        assert!(locked);

        let priced_order = ctx.priced_orders_rx.try_recv().unwrap();
        assert!(priced_order.target_timestamp.is_some());
    }
}
