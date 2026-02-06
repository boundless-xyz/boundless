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
#![allow(missing_docs)]

pub mod config;
pub(crate) mod local_executor;
pub mod prover;
pub(crate) mod requestor_pricing;
pub mod storage;

#[cfg(not(feature = "prover_utils"))]
pub use config::MarketConfig;
#[cfg(feature = "prover_utils")]
pub use config::{
    defaults as config_defaults, BatcherConfig, Config, MarketConfig, OrderCommitmentPriority,
    OrderPricingPriority, ProverConfig,
};

use crate::{
    contracts::{
        FulfillmentData, Predicate, PredicateType, ProofRequest, RequestError, RequestInputType,
    },
    input::GuestEnv,
    price_oracle::{scale_decimals, Amount},
    selector::{is_blake3_groth16_selector, SupportedSelectors},
    storage::StorageDownloader,
    util::now_timestamp,
};
use alloy::{
    primitives::{
        utils::{format_ether, format_units},
        Address, Bytes, FixedBytes, U256,
    },
    uint,
};
use anyhow::Context;
use hex::FromHex;
use moka::future::Cache;
use prover::ProverObj;
use risc0_zkvm::sha::Digest as Risc0Digest;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::HashSet,
    fmt,
    sync::{Arc, OnceLock},
};
use thiserror::Error;
use OrderPricingOutcome::Skip;

const ONE_MILLION: U256 = uint!(1_000_000_U256);

/// Execution limit reasoning details.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProveLimitReason {
    EthPricing {
        max_price: U256,
        gas_cost: U256,
        mcycle_price_eth: U256,
        config_mcycle_price: String,
    },
    CollateralPricing {
        collateral_reward: String,
        mcycle_price_collateral: String,
    },
    ConfigCap {
        max_mcycles: u64,
    },
    DeadlineCap {
        time_remaining_secs: u64,
        peak_prove_khz: u64,
    },
}

impl fmt::Display for ProveLimitReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ProveLimitReason::CollateralPricing { collateral_reward, mcycle_price_collateral } => {
                write!(
                    f,
                    "collateral pricing: order collateral reward {} / {} mcycle_price_collateral_token config",
                    collateral_reward,
                    mcycle_price_collateral,
                )
            }
            ProveLimitReason::EthPricing {
                max_price,
                gas_cost,
                mcycle_price_eth,
                config_mcycle_price,
            } => {
                write!(
                    f,
                    "pricing: (order maxPrice {} - gas {}) / min_mcycle_price {} ({} ETH per Mcycle)",
                    format_ether(*max_price),
                    format_ether(*gas_cost),
                    config_mcycle_price,
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

/// Errors returned by pricing logic.
#[derive(Error, Debug, Clone)]
#[non_exhaustive]
pub enum OrderPricingError {
    /// Failed to fetch or upload input.
    #[error("failed to fetch / push input: {0:#}")]
    FetchInputErr(#[source] Arc<anyhow::Error>),
    /// Failed to fetch or upload image.
    #[error("failed to fetch / push image: {0:#}")]
    FetchImageErr(#[source] Arc<anyhow::Error>),
    /// Invalid request.
    #[error("invalid request: {0}")]
    RequestError(Arc<RequestError>),
    /// RPC error.
    #[error("RPC error: {0:#}")]
    RpcErr(Arc<anyhow::Error>),
    /// Unexpected error.
    #[error("Unexpected error: {0:#}")]
    UnexpectedErr(Arc<anyhow::Error>),
}

impl From<anyhow::Error> for OrderPricingError {
    fn from(err: anyhow::Error) -> Self {
        OrderPricingError::UnexpectedErr(Arc::new(err))
    }
}

impl From<RequestError> for OrderPricingError {
    fn from(err: RequestError) -> Self {
        OrderPricingError::RequestError(Arc::new(err))
    }
}

/// Order fulfillment mode.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum FulfillmentType {
    LockAndFulfill,
    FulfillAfterLockExpire,
    // Currently not supported
    FulfillWithoutLocking,
}

/// Order request from the network.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct OrderRequest {
    pub request: ProofRequest,
    pub client_sig: Bytes,
    pub fulfillment_type: FulfillmentType,
    pub boundless_market_address: Address,
    pub chain_id: u64,
    pub image_id: Option<String>,
    pub input_id: Option<String>,
    pub total_cycles: Option<u64>,
    pub target_timestamp: Option<u64>,
    pub expire_timestamp: Option<u64>,
    #[serde(skip)]
    cached_id: OnceLock<String>,
}

impl OrderRequest {
    pub fn new(
        request: ProofRequest,
        client_sig: Bytes,
        fulfillment_type: FulfillmentType,
        boundless_market_address: Address,
        chain_id: u64,
    ) -> Self {
        Self {
            request,
            client_sig,
            fulfillment_type,
            boundless_market_address,
            chain_id,
            image_id: None,
            input_id: None,
            total_cycles: None,
            target_timestamp: None,
            expire_timestamp: None,
            cached_id: OnceLock::new(),
        }
    }

    pub fn id(&self) -> String {
        self.cached_id
            .get_or_init(|| {
                let signing_hash = self
                    .request
                    .signing_hash(self.boundless_market_address, self.chain_id)
                    .unwrap_or(FixedBytes::ZERO);
                format!("0x{:x}-{}-{:?}", self.request.id, signing_hash, self.fulfillment_type)
            })
            .clone()
    }

    pub fn expiry(&self) -> u64 {
        match self.fulfillment_type {
            FulfillmentType::LockAndFulfill => self.request.lock_expires_at(),
            FulfillmentType::FulfillAfterLockExpire => self.request.expires_at(),
            FulfillmentType::FulfillWithoutLocking => self.request.expires_at(),
        }
    }
}

impl std::fmt::Display for OrderRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let total_mcycles = if self.total_cycles.is_some() {
            format!(" ({} mcycles)", self.total_cycles.unwrap() / 1_000_000)
        } else {
            "".to_string()
        };
        write!(f, "{}{} [{}]", self.id(), total_mcycles, format_expiries(&self.request))
    }
}

/// Outcome of order pricing.
#[derive(Debug)]
#[cfg_attr(not(feature = "prover_utils"), allow(dead_code))]
pub enum OrderPricingOutcome {
    /// Order should be locked and proving commence after lock is secured.
    Lock {
        total_cycles: u64,
        target_timestamp_secs: u64,
        expiry_secs: u64,
        target_mcycle_price: U256,
        max_mcycle_price: U256,
        config_min_mcycle_price: U256,
        current_mcycle_price: U256,
    },
    /// Do not lock the order, but consider proving and fulfilling it after the lock expires.
    ProveAfterLockExpire {
        total_cycles: u64,
        lock_expire_timestamp_secs: u64,
        expiry_secs: u64,
        mcycle_price: U256,
        config_min_mcycle_price: U256,
    },
    /// Do not accept engage order.
    Skip { reason: String },
}

/// Value type for preflight cache.
#[derive(Clone, Debug)]
pub enum PreflightCacheValue {
    Success { exec_session_id: String, cycle_count: u64, image_id: String, input_id: String },
    Skip { cached_limit: u64 },
}

/// Input type for preflight cache.
#[derive(Hash, Eq, PartialEq, Clone, Debug)]
pub enum InputCacheKey {
    /// URL-based input.
    Url(String),
    /// Hash-based input (for inline data).
    Hash([u8; 32]),
}

/// Key type for the preflight cache.
#[derive(Hash, Eq, PartialEq, Clone, Debug)]
pub struct PreflightCacheKey {
    /// The predicate data.
    pub predicate_data: Vec<u8>,
    /// The input cache key.
    pub input: InputCacheKey,
}

/// Cache for preflight results to avoid duplicate computations.
pub type PreflightCache = Arc<Cache<PreflightCacheKey, PreflightCacheValue>>;

/// Upload an image to the prover using the provided downloader.
///
/// This is a standalone function (not a trait method) so it can be called from inside
/// async closures like `try_get_with` without capturing `&self`.
async fn upload_image_with_downloader(
    prover: &ProverObj,
    image_url: &str,
    predicate: &crate::contracts::RequestPredicate,
    downloader: &(dyn StorageDownloader + Send + Sync),
) -> anyhow::Result<String> {
    let predicate = Predicate::try_from(predicate.clone()).context("Failed to parse predicate")?;

    let image_id_str = predicate.image_id().map(|image_id| image_id.to_string());

    // Check if prover already has the image cached
    if let Some(ref image_id_str) = image_id_str {
        if prover.has_image(image_id_str).await? {
            tracing::debug!("Skipping program upload for cached image ID: {image_id_str}");
            return Ok(image_id_str.clone());
        }
    }

    tracing::debug!("Fetching program from URI {image_url}");
    let image_data = downloader
        .download(image_url)
        .await
        .with_context(|| format!("Failed to fetch image URI: {image_url}"))?;

    let image_id =
        risc0_zkvm::compute_image_id(&image_data).context("Failed to compute image ID")?;

    if let Some(ref expected_image_id_str) = image_id_str {
        let expected_image_id = risc0_zkvm::sha::Digest::from_hex(expected_image_id_str)?;
        if image_id != expected_image_id {
            anyhow::bail!(
                "image ID does not match requirements; expect {}, got {}",
                expected_image_id,
                image_id
            );
        }
    }

    let image_id_str = image_id.to_string();

    tracing::debug!("Uploading program with image ID {image_id_str} to prover");
    prover.upload_image(&image_id_str, image_data).await?;

    Ok(image_id_str)
}

/// Upload input data to the prover (from inline data or URL) using the provided downloader.
///
/// This is a standalone function (not a trait method) so it can be called from inside
/// async closures like `try_get_with` without capturing `&self`.
///
/// If `is_priority_requestor` is true, size limits are bypassed when fetching from URLs.
async fn upload_input_with_downloader(
    prover: &ProverObj,
    input_type: crate::contracts::RequestInputType,
    input_data: &Bytes,
    downloader: &(dyn StorageDownloader + Send + Sync),
    is_priority_requestor: bool,
) -> anyhow::Result<String> {
    match input_type {
        crate::contracts::RequestInputType::Inline => {
            let stdin = GuestEnv::decode(input_data).context("Failed to decode input")?.stdin;
            prover.upload_input(stdin).await.map_err(|e| anyhow::anyhow!("{}", e))
        }
        crate::contracts::RequestInputType::Url => {
            let input_url =
                std::str::from_utf8(input_data).context("input url is not valid utf8")?;

            tracing::debug!("Fetching input from URI {input_url}");
            let raw_input = if is_priority_requestor {
                downloader.download_with_limit(input_url, usize::MAX).await
            } else {
                downloader.download(input_url).await
            }
            .with_context(|| format!("Failed to fetch input URI: {input_url}"))?;

            let stdin =
                GuestEnv::decode(&raw_input).context("Failed to decode input from URL")?.stdin;

            prover.upload_input(stdin).await.map_err(|e| anyhow::anyhow!("{}", e))
        }
        crate::contracts::RequestInputType::__Invalid => {
            anyhow::bail!("Invalid input type")
        }
    }
}

/// Context required by order pricing.
#[allow(async_fn_in_trait)]
pub trait OrderPricingContext {
    fn market_config(&self) -> Result<MarketConfig, OrderPricingError>;
    fn supported_selectors(&self) -> &SupportedSelectors;
    fn preflight_cache(&self) -> &PreflightCache;
    fn collateral_token_decimals(&self) -> u8;
    fn format_collateral(&self, value: U256) -> String {
        format_units(value, self.collateral_token_decimals()).unwrap_or_else(|_| "?".to_string())
    }
    /// Returns the set of denied requestor addresses, if any are configured.
    fn denied_requestor_addresses(&self) -> Result<Option<HashSet<Address>>, OrderPricingError> {
        Ok(None)
    }
    fn check_requestor_allowed(
        &self,
        order: &OrderRequest,
        denied_addresses_opt: Option<&HashSet<Address>>,
    ) -> Result<Option<OrderPricingOutcome>, OrderPricingError>;
    async fn check_request_available(
        &self,
        order: &OrderRequest,
    ) -> Result<Option<OrderPricingOutcome>, OrderPricingError>;
    #[cfg(feature = "prover_utils")]
    async fn estimate_gas_to_fulfill_pending(&self) -> Result<u64, OrderPricingError>;
    fn is_priority_requestor(&self, client_addr: &Address) -> bool;
    async fn check_available_balances(
        &self,
        order: &OrderRequest,
        order_gas_cost: U256,
        lock_expired: bool,
        lockin_collateral: U256,
    ) -> Result<Option<OrderPricingOutcome>, OrderPricingError>;
    async fn current_gas_price(&self) -> Result<u128, OrderPricingError>;

    /// Convert an Amount to ETH (using the price oracle).
    async fn convert_to_eth(&self, amount: &Amount) -> Result<Amount, OrderPricingError>;

    /// Convert an Amount to ZKC using the price oracle.
    async fn convert_to_zkc(&self, amount: &Amount) -> Result<Amount, OrderPricingError>;

    /// Access to the prover for preflight operations.
    fn prover(&self) -> &ProverObj;

    /// Access to the downloader for fetching images and inputs.
    /// Returns an Arc so it can be cloned into async closures.
    fn downloader(&self) -> Arc<dyn StorageDownloader + Send + Sync>;

    /// Fetch data from a URI using the downloader.
    #[cfg(feature = "prover_utils")]
    async fn fetch_url(&self, url: &str) -> Result<Vec<u8>, OrderPricingError> {
        self.downloader()
            .download(url)
            .await
            .map_err(|e| OrderPricingError::FetchInputErr(Arc::new(e.into())))
    }

    /// Upload an image from a URL to the prover using the downloader.
    #[cfg(feature = "prover_utils")]
    async fn upload_image(
        &self,
        image_url: &str,
        predicate: &crate::contracts::RequestPredicate,
    ) -> Result<String, OrderPricingError> {
        upload_image_with_downloader(
            self.prover(),
            image_url,
            predicate,
            self.downloader().as_ref(),
        )
        .await
        .map_err(|e| OrderPricingError::FetchImageErr(Arc::new(e)))
    }

    /// Upload input data to the prover (from inline data or URL) using the downloader.
    #[cfg(feature = "prover_utils")]
    async fn upload_input(&self, order: &OrderRequest) -> Result<String, OrderPricingError> {
        let is_priority = self.is_priority_requestor(&order.request.client_address());
        upload_input_with_downloader(
            self.prover(),
            order.request.input.inputType,
            &order.request.input.data,
            self.downloader().as_ref(),
            is_priority,
        )
        .await
        .map_err(|e| OrderPricingError::FetchInputErr(Arc::new(e)))
    }

    /// Execute preflight for an order.
    async fn preflight_execute(
        &self,
        order: &OrderRequest,
        exec_limit_cycles: u64,
        cache_key: PreflightCacheKey,
    ) -> Result<PreflightCacheValue, OrderPricingError> {
        let order_id = order.id();

        // Clone all data needed inside the closure
        let prover = self.prover().clone();
        let downloader = self.downloader();
        let cache = self.preflight_cache().clone();
        let image_url = order.request.imageUrl.clone();
        let predicate = order.request.requirements.predicate.clone();
        let input_type = order.request.input.inputType;
        let input_data = order.request.input.data.clone();
        let order_id_clone = order_id.clone();
        let is_priority = self.is_priority_requestor(&order.request.client_address());

        // Multiple concurrent calls of this coalesce into a single execution.
        // https://docs.rs/moka/latest/moka/future/struct.Cache.html#concurrent-calls-on-the-same-key
        let result = cache
            .try_get_with(cache_key, async move {
                tracing::trace!(
                    "Starting preflight execution of {order_id_clone} with limit of {exec_limit_cycles} cycles"
                );

                // Upload image from URL using downloader
                let image_id =
                    upload_image_with_downloader(&prover, &image_url, &predicate, downloader.as_ref())
                        .await
                        .map_err(|e| OrderPricingError::FetchImageErr(Arc::new(e)))?;

                // Upload input using downloader
                let input_id =
                    upload_input_with_downloader(&prover, input_type, &input_data, downloader.as_ref(), is_priority)
                        .await
                        .map_err(|e| OrderPricingError::FetchInputErr(Arc::new(e)))?;

                match prover
                    .preflight(&image_id, &input_id, vec![], Some(exec_limit_cycles), &order_id_clone)
                    .await
                {
                    Ok(res) => {
                        tracing::debug!(
                            "Preflight execution of {order_id_clone} with session id {} and {} mcycles completed",
                            res.id,
                            res.stats.total_cycles / 1_000_000
                        );
                        Ok(PreflightCacheValue::Success {
                            exec_session_id: res.id,
                            cycle_count: res.stats.total_cycles,
                            image_id,
                            input_id,
                        })
                    }
                    Err(err) => {
                        let err_msg = err.to_string();
                        if err_msg.contains("Session limit exceeded")
                            || err_msg.contains("Execution stopped intentionally due to session limit")
                        {
                            tracing::debug!(
                                "Skipping order {order_id_clone} due to intentional execution limit of {exec_limit_cycles}"
                            );
                            Ok(PreflightCacheValue::Skip { cached_limit: exec_limit_cycles })
                        } else if err_msg.contains("Guest panicked") || err_msg.contains("GuestPanic") {
                            tracing::debug!(
                                "Skipping order {order_id_clone} due to guest panic: {}",
                                err_msg
                            );
                            Ok(PreflightCacheValue::Skip { cached_limit: u64::MAX })
                        } else {
                            Err(OrderPricingError::UnexpectedErr(Arc::new(err.into())))
                        }
                    }
                }
            })
            .await
            .map_err(|e| (*e).clone())?;

        Ok(result)
    }

    /// Fetch the journal from a preflight execution.
    async fn preflight_journal(&self, exec_session_id: &str) -> Result<Vec<u8>, OrderPricingError> {
        self.prover()
            .get_preflight_journal(exec_session_id)
            .await
            .map_err(|e| OrderPricingError::UnexpectedErr(Arc::new(e.into())))?
            .ok_or_else(|| {
                OrderPricingError::UnexpectedErr(Arc::new(anyhow::anyhow!(
                    "Preflight journal not found"
                )))
            })
    }
    async fn price_order(
        &self,
        order: &mut OrderRequest,
    ) -> Result<OrderPricingOutcome, OrderPricingError> {
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

        let config = self.market_config()?;
        let min_deadline = config.min_deadline;
        let denied_addresses_opt = self.denied_requestor_addresses()?;

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
        if let Some(outcome) = self.check_requestor_allowed(order, denied_addresses_opt.as_ref())? {
            return Ok(outcome);
        }

        if !self.supported_selectors().is_supported(order.request.requirements.selector) {
            return Ok(Skip {
                reason: format!(
                    "unsupported selector requirement. Requested: {:x}. Supported: {:?}",
                    order.request.requirements.selector,
                    self.supported_selectors()
                        .selectors
                        .iter()
                        .map(|(k, v)| format!("{k:x} ({v:?})"))
                        .collect::<Vec<_>>()
                ),
            });
        };

        // Check if the collateral is sane and if we can afford it
        // For lock expired orders, we don't check the max collateral because we can't lock those orders.
        let max_collateral_amount = &config.max_collateral;
        let max_collateral_zkc = self.convert_to_zkc(max_collateral_amount).await?;
        // Convert from Asset ZKC decimals (18) to contract collateral token decimals (6 or 18 depending on chain)
        let max_collateral: U256 = scale_decimals(
            max_collateral_zkc.value,
            max_collateral_zkc.asset.decimals(),
            self.collateral_token_decimals(),
        );

        if !lock_expired && lockin_collateral > max_collateral {
            return Ok(Skip {
                reason: format!(
                    "order collateral requirement exceeds max_collateral config {} > {} ({})",
                    self.format_collateral(lockin_collateral),
                    self.format_collateral(max_collateral),
                    max_collateral_amount,
                ),
            });
        }

        // Short circuit if the order cannot be acted on.
        if let Some(outcome) = self.check_request_available(order).await? {
            return Ok(outcome);
        }

        // Check that we have both enough staking tokens to collateral, and enough gas tokens to lock and fulfil
        // NOTE: We use the current gas price and a rough heuristic on gas costs. Its possible that
        // gas prices may go up (or down) by the time its time to fulfill. This does not aim to be
        // a tight estimate, although improving this estimate will allow for a more profit.
        let gas_price = self.current_gas_price().await?;
        let order_gas = if lock_expired {
            // No need to include lock gas if its a lock expired order
            U256::from(
                estimate_gas_to_fulfill(&config, self.supported_selectors(), &order.request)
                    .await?,
            )
        } else {
            U256::from(
                estimate_gas_to_lock(&config, order).await?
                    + estimate_gas_to_fulfill(&config, self.supported_selectors(), &order.request)
                        .await?,
            )
        };
        let order_gas_cost = U256::from(gas_price) * order_gas;
        tracing::debug!(
            "Estimated {order_gas} gas to {} order {order_id}; {} ether @ {} gwei",
            if lock_expired { "fulfill" } else { "lock and fulfill" },
            format_ether(order_gas_cost),
            format_units(gas_price, "gwei").unwrap()
        );

        if let Some(outcome) = self
            .check_available_balances(order, order_gas_cost, lock_expired, lockin_collateral)
            .await?
        {
            return Ok(outcome);
        }

        // Calculate exec limit (handles priority requestors and config internally)
        let (exec_limit_cycles, prove_limit, prove_limit_reason) =
            self.calculate_exec_limits(order, order_gas_cost).await?;

        if prove_limit < 2 {
            // Exec limit is based on user cycles, and 2 is the minimum number of user cycles for a
            // provable execution.
            // TODO when/if total cycle limit is allowed in future, update this to be total cycle min
            return Ok(Skip {
                reason: format!("cycle limit hit from max reward: {} cycles", prove_limit),
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
                    .map_err(|e| OrderPricingError::FetchInputErr(Arc::new(e)))?
                    .to_string();
                PreflightCacheKey { predicate_data, input: InputCacheKey::Url(input_url) }
            }
            RequestInputType::Inline => {
                // For inline inputs, use SHA256 hash of the data
                let mut hasher = Sha256::new();
                sha2::Digest::update(&mut hasher, &order.request.input.data);
                let input_hash: [u8; 32] = hasher.finalize().into();
                PreflightCacheKey { predicate_data, input: InputCacheKey::Hash(input_hash) }
            }
            RequestInputType::__Invalid => {
                return Err(OrderPricingError::UnexpectedErr(Arc::new(anyhow::anyhow!(
                    "Unknown input type: {:?}",
                    order.request.input.inputType
                ))));
            }
        };

        let preflight_result = loop {
            let result =
                self.preflight_execute(order, exec_limit_cycles, cache_key.clone()).await?;
            if let PreflightCacheValue::Skip { cached_limit } = result {
                if cached_limit < exec_limit_cycles {
                    self.preflight_cache().invalidate(&cache_key).await;
                    continue;
                }
            }
            break result;
        };

        let (exec_session_id, cycle_count, image_id) = match preflight_result {
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
            PreflightCacheValue::Skip { cached_limit } => {
                return Ok(Skip {
                    reason: format!(
                        "order preflight execution limit hit with {cached_limit} cycles - limited by {prove_limit_reason}"
                    ),
                });
            }
        };

        // If a max_mcycle_limit is configured check if the order is over that limit
        if cycle_count > prove_limit {
            // If the preflight execution has completed, but for the variant is rejected,
            // provide the config value that needs to be updated in order to have accepted.
            let config_info = match &prove_limit_reason {
                ProveLimitReason::EthPricing {
                    max_price,
                    gas_cost,
                    mcycle_price_eth,
                    config_mcycle_price,
                } => {
                    let max_price_gas_adjusted = max_price.saturating_sub(*gas_cost);
                    let required_price_per_mcycle = max_price_gas_adjusted
                        .saturating_mul(ONE_MILLION)
                        / U256::from(cycle_count);
                    let required_price_per_mcycle_ignore_gas =
                        max_price.saturating_mul(ONE_MILLION) / U256::from(cycle_count);
                    format!(
                        "min_mcycle_price set to {} ({} ETH/Mcycle) in config, order requires min_mcycle_price <= {} ETH/Mcycle to be considered (gas cost: {} ETH, ignoring gas requires min {} ETH/Mcycle)",
                        config_mcycle_price,
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
                        reward.saturating_mul(ONE_MILLION) / U256::from(cycle_count);
                    format!(
                        "min_mcycle_price_collateral_token set to {} ZKC/Mcycle in config, order requires min_mcycle_price_collateral_token <= {} ZKC/Mcycle to be considered",
                        mcycle_price_collateral,
                        self.format_collateral(required_collateral_price)
                    )
                }
                ProveLimitReason::ConfigCap { max_mcycles } => {
                    let required_mcycles = cycle_count.div_ceil(1_000_000);
                    format!(
                        "max_mcycle_limit set to {} Mcycles in config, order requires max_mcycle_limit >= {} Mcycles to be considered",
                        max_mcycles, required_mcycles
                    )
                }
                ProveLimitReason::DeadlineCap { time_remaining_secs, peak_prove_khz } => {
                    let denom = time_remaining_secs.saturating_mul(1_000);
                    let required_khz = cycle_count.div_ceil(denom);
                    format!(
                        "peak_prove_khz set to {} kHz in config, order requires peak_prove_khz >= {} kHz to be considered",
                        peak_prove_khz, required_khz
                    )
                }
            };

            return Ok(Skip {
                reason: format!(
                    "order with {cycle_count} cycles above limit of {prove_limit} cycles - {config_info}"
               ),
            });
        }

        order.total_cycles = Some(cycle_count);

        if config.min_mcycle_limit > 0 {
            let min_cycles = config.min_mcycle_limit.saturating_mul(1_000_000);
            if cycle_count < min_cycles {
                tracing::debug!(
                        "Order {order_id} skipped due to min_mcycle_limit config: {} cycles < {} cycles",
                        cycle_count,
                        min_cycles
                    );
                return Ok(Skip {
                        reason: format!(
                            "order with {} cycles below min limit of {} cycles - min_mcycle_limit set to {} Mcycles in config",
                            cycle_count,
                            min_cycles,
                            config.min_mcycle_limit,
                        ),
                    });
            }
        }

        let journal = self.preflight_journal(&exec_session_id).await?;
        let order_predicate_type = order.request.requirements.predicate.predicateType;
        if matches!(order_predicate_type, PredicateType::PrefixMatch | PredicateType::DigestMatch)
            && journal.len() > config.max_journal_bytes
        {
            return Ok(OrderPricingOutcome::Skip {
                reason: format!(
                    "order journal larger than set limit ({} > {})",
                    journal.len(),
                    config.max_journal_bytes
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
            .map_err(|e| OrderPricingError::RequestError(Arc::new(e.into())))?;
        let eval_data = if is_blake3_groth16_selector(order.request.requirements.selector) {
            // These proofs must have no journal delivery because they cannot be authenticated on chain.
            FulfillmentData::None
        } else {
            FulfillmentData::from_image_id_and_journal(
                Risc0Digest::from_hex(image_id).context("Failed to parse image ID")?,
                journal,
            )
        };
        if predicate.eval(&eval_data).is_none() {
            return Ok(Skip { reason: "order predicate check failed".to_string() });
        }

        // For lock_expired orders, evaluate based on collateral
        if lock_expired {
            // Reward for the order is a fraction of the collateral once the lock has expired
            let price = order.request.offer.collateral_reward_if_locked_and_not_fulfilled();
            let mcycle_price_in_collateral_tokens =
                price.saturating_mul(ONE_MILLION) / U256::from(cycle_count);

            // Get the configured price as Amount
            let config_min_mcycle_price_collateral_token =
                &config.min_mcycle_price_collateral_token;

            // Convert to ZKC (handles USD via price oracle)
            let config_min_mcycle_price_zkc =
                self.convert_to_zkc(config_min_mcycle_price_collateral_token).await?;

            // Scale from Asset ZKC decimals (18) to contract collateral token decimals
            let config_min_mcycle_price_collateral_tokens: U256 = scale_decimals(
                config_min_mcycle_price_zkc.value,
                config_min_mcycle_price_zkc.asset.decimals(),
                self.collateral_token_decimals(),
            );

            tracing::debug!(
                "Order price: {} (collateral tokens) - cycles: {} - mcycle price: {} (collateral tokens), config_min_mcycle_price_collateral_tokens: {} (collateral tokens)",
                format_ether(price),
                cycle_count,
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

            Ok(OrderPricingOutcome::ProveAfterLockExpire {
                total_cycles: cycle_count,
                lock_expire_timestamp_secs: order.request.offer.rampUpStart
                    + order.request.offer.lockTimeout as u64,
                expiry_secs: order.request.offer.rampUpStart + order.request.offer.timeout as u64,
                mcycle_price: mcycle_price_in_collateral_tokens,
                config_min_mcycle_price: config_min_mcycle_price_collateral_tokens,
            })
        } else {
            // For lockable orders, evaluate based on ETH price
            let config_min_mcycle_price_amount = &config.min_mcycle_price;

            // Convert configured price to ETH (i.e., handles USD via price oracle)
            let config_min_mcycle_price_eth =
                self.convert_to_eth(config_min_mcycle_price_amount).await?;
            let config_min_mcycle_price: U256 = config_min_mcycle_price_eth.value;

            let order_id = order.id();

            let mcycle_price_min = U256::from(order.request.offer.minPrice)
                .saturating_sub(order_gas_cost)
                .saturating_mul(ONE_MILLION)
                / U256::from(cycle_count);
            let mcycle_price_max = U256::from(order.request.offer.maxPrice)
                .saturating_sub(order_gas_cost)
                .saturating_mul(ONE_MILLION)
                / U256::from(cycle_count);

            tracing::debug!(
                "Order {order_id} price: {}-{} ETH, {}-{} ETH per mcycle (min_mcycle_price: {}), {} collateral required, {} ETH gas cost",
                format_ether(U256::from(order.request.offer.minPrice)),
                format_ether(U256::from(order.request.offer.maxPrice)),
                format_ether(mcycle_price_min),
                format_ether(mcycle_price_max),
                config_min_mcycle_price_amount,
                self.format_collateral(order.request.offer.lockCollateral),
                format_ether(order_gas_cost),
            );

            // Skip the order if it will never be worth it
            if mcycle_price_max < config_min_mcycle_price {
                return Ok(OrderPricingOutcome::Skip {
                    reason: format!(
                        "order max price {} is less than mcycle_price config {} ({} ETH)",
                        format_ether(U256::from(order.request.offer.maxPrice)),
                        config_min_mcycle_price_amount,
                        format_ether(config_min_mcycle_price),
                    ),
                });
            }

            let current_mcycle_price = order
                .request
                .offer
                .price_at(now_timestamp())
                .context("Failed to get current mcycle price")?;
            let (target_mcycle_price, target_timestamp_secs) = if mcycle_price_min
                >= config_min_mcycle_price
            {
                tracing::info!(
                    "Selecting order {order_id} at price {} (min_mcycle_price: {} = {} ETH) - ASAP",
                    format_ether(current_mcycle_price),
                    config_min_mcycle_price_amount,
                    format_ether(config_min_mcycle_price)
                );
                (mcycle_price_min, 0) // Schedule the lock ASAP
            } else {
                let target_min_price = config_min_mcycle_price
                    .saturating_mul(U256::from(cycle_count))
                    .div_ceil(ONE_MILLION)
                    + order_gas_cost;
                tracing::debug!(
                        "Order {order_id} minimum profitable price: {} ETH (min_mcycle_price: {} = {} ETH)",
                        format_ether(target_min_price),
                        config_min_mcycle_price_amount,
                        format_ether(config_min_mcycle_price)
                    );

                let target_time = order
                    .request
                    .offer
                    .time_at_price(target_min_price)
                    .context("Failed to get target price timestamp")?;
                (target_min_price, target_time)
            };

            let expiry_secs =
                order.request.offer.rampUpStart + order.request.offer.lockTimeout as u64;

            Ok(OrderPricingOutcome::Lock {
                total_cycles: cycle_count,
                target_timestamp_secs,
                expiry_secs,
                target_mcycle_price,
                max_mcycle_price: mcycle_price_max,
                config_min_mcycle_price,
                current_mcycle_price,
            })
        }
    }

    /// Calculates the cycle limit for the preflight and also for the max cycles that this specific
    /// order variant will consider proving for.
    ///
    /// The reason for calculating both preflight and prove limits is to execute the order with
    /// a large enough cycle limit for both lock and fulfill orders as well as for if the order
    /// expires and to prove after lock expiry so that the execution can be cached and only happen
    /// once. The prove limit is the limit for this specific order variant and decides the max
    /// cycles the order can be for the prover to decide to commit to proving it.
    async fn calculate_exec_limits(
        &self,
        order: &OrderRequest,
        order_gas_cost: U256,
    ) -> Result<(u64, u64, ProveLimitReason), OrderPricingError> {
        // Derive parameters from order
        let order_id = order.id();
        let is_fulfill_after_lock_expire =
            order.fulfillment_type == FulfillmentType::FulfillAfterLockExpire;
        let now = now_timestamp();
        let request_expiration = order.expiry();
        let lock_expiry = order.request.lock_expires_at();
        let order_expiry = order.request.expires_at();
        let config = self.market_config()?;
        let min_mcycle_price_amount = &config.min_mcycle_price;

        // Convert configured price to ETH (handles USD via price oracle)
        let min_mcycle_price_eth = self.convert_to_eth(min_mcycle_price_amount).await?;
        let min_mcycle_price: U256 = min_mcycle_price_eth.value;

        // Get the configured price as Amount
        let config_min_mcycle_price_collateral_token = &config.min_mcycle_price_collateral_token;

        // Convert to ZKC (handles USD via price oracle)
        let config_min_mcycle_price_zkc =
            self.convert_to_zkc(config_min_mcycle_price_collateral_token).await?;

        // Scale from Asset ZKC decimals (18) to contract collateral token decimals
        let min_mcycle_price_collateral_tokens: U256 = scale_decimals(
            config_min_mcycle_price_zkc.value,
            config_min_mcycle_price_zkc.asset.decimals(),
            self.collateral_token_decimals(),
        );

        // Pricing based cycle limits: Calculate the cycle limit based on collateral price
        let collateral_based_limit = if min_mcycle_price_collateral_tokens == U256::ZERO {
            tracing::info!("min_mcycle_price_collateral_token is 0, setting unlimited exec limit");
            u64::MAX
        } else {
            let price = order.request.offer.collateral_reward_if_locked_and_not_fulfilled();

            let initial_collateral_based_limit =
                (price.saturating_mul(ONE_MILLION).div_ceil(min_mcycle_price_collateral_tokens))
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
            mcycle_price_collateral: self.format_collateral(min_mcycle_price_collateral_tokens),
            collateral_reward: self.format_collateral(
                order.request.offer.collateral_reward_if_locked_and_not_fulfilled(),
            ),
        };

        // If lock and fulfill, potentially increase that to ETH-based value if higher
        if !is_fulfill_after_lock_expire {
            // Calculate eth-based limit for lock and fulfill orders
            let eth_based_limit: u64 = if min_mcycle_price == U256::ZERO {
                tracing::info!("min_mcycle_price is 0, setting unlimited exec limit");
                u64::MAX
            } else {
                let limit: U256 = U256::from(order.request.offer.maxPrice)
                    .saturating_sub(order_gas_cost)
                    .saturating_mul(ONE_MILLION)
                    / min_mcycle_price;
                limit.try_into().unwrap_or(u64::MAX)
            };

            if eth_based_limit > collateral_based_limit {
                // Eth based limit is higher, use that for both preflight and prove
                tracing::debug!("Order {order_id} eth based limit ({eth_based_limit}) from min_mcycle_price {} > collateral based limit ({collateral_based_limit}), using eth based limit for both preflight and prove", min_mcycle_price_amount);
                preflight_limit = eth_based_limit;
                prove_limit = eth_based_limit;
            } else {
                // Otherwise lower the prove cycle limit for this order variant
                tracing::debug!("Order {order_id} eth based limit ({eth_based_limit}) from min_mcycle_price {} < collateral based limit ({collateral_based_limit}), using eth based limit for prove", min_mcycle_price_amount);
                prove_limit = eth_based_limit;
            }
            prove_limit_reason = ProveLimitReason::EthPricing {
                max_price: U256::from(order.request.offer.maxPrice),
                gas_cost: order_gas_cost,
                mcycle_price_eth: min_mcycle_price,
                config_mcycle_price: min_mcycle_price_amount.to_string(),
            };
        }

        debug_assert!(
            preflight_limit >= prove_limit,
            "preflight_limit ({preflight_limit}) < prove_limit ({prove_limit})",
        );

        // Apply max mcycle limit cap
        // Check if priority requestor address - skip all exec limit calculations
        let client_addr = order.request.client_address();
        let skip_mcycle_limit = self.is_priority_requestor(&client_addr);
        if skip_mcycle_limit {
            tracing::debug!("Order {order_id} exec limit config ignored due to client {} being part of priority requestors.", client_addr);
        }

        if !skip_mcycle_limit {
            let config_cycle_limit = config.max_mcycle_limit.saturating_mul(1_000_000);
            if prove_limit > config_cycle_limit {
                tracing::debug!(
                    "Order {order_id} prove limit capped by max_mcycle_limit config: {} -> {} cycles",
                    prove_limit,
                    config_cycle_limit
                );
                prove_limit = config_cycle_limit;
                preflight_limit = config_cycle_limit;
                prove_limit_reason =
                    ProveLimitReason::ConfigCap { max_mcycles: config.max_mcycle_limit };
            } else if preflight_limit > config_cycle_limit {
                preflight_limit = config_cycle_limit;
            }
        }

        // Apply timing constraints based on peak prove khz
        if let Some(peak_prove_khz) = config.peak_prove_khz {
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
}

fn calculate_max_cycles_for_time(peak_prove_khz: u64, time_remaining_secs: u64) -> u64 {
    peak_prove_khz.saturating_mul(time_remaining_secs) * 1_000
}

fn format_expiries(request: &ProofRequest) -> String {
    let now: i64 = now_timestamp().try_into().unwrap();
    let lock_expires_at: i64 = request.lock_expires_at().try_into().unwrap();
    let lock_expires_delta = lock_expires_at - now;
    let lock_expires_delta_str = if lock_expires_delta > 0 {
        format!("{lock_expires_delta} seconds from now")
    } else {
        format!("{} seconds ago", lock_expires_delta.abs())
    };
    let expires_at: i64 = request.expires_at().try_into().unwrap();
    let expires_delta = expires_at - now;
    let expires_delta_str = if expires_delta > 0 {
        format!("{expires_delta} seconds from now")
    } else {
        format!("{} seconds ago", expires_delta.abs())
    };
    format!("Lock expires {lock_expires_delta_str}, expires {expires_delta_str}")
}

async fn estimate_gas_to_fulfill(
    config: &MarketConfig,
    supported_selectors: &SupportedSelectors,
    request: &ProofRequest,
) -> anyhow::Result<u64> {
    use crate::selector::ProofType;

    let selector = request.requirements.selector;
    if !supported_selectors.is_supported(selector) {
        anyhow::bail!("unsupported selector requirement: {selector:x}");
    }

    let mut estimate = config.fulfill_gas_estimate;

    // Add gas for orders that make use of the callbacks feature.
    estimate += u64::try_from(
        request
            .requirements
            .callback
            .as_option()
            .map(|callback| callback.gasLimit)
            .unwrap_or(alloy::primitives::aliases::U96::ZERO),
    )?;

    estimate += match supported_selectors.proof_type(selector).context("unsupported selector")? {
        ProofType::Any | ProofType::Inclusion => 0,
        ProofType::Groth16 | ProofType::Blake3Groth16 => config.groth16_verify_gas_estimate,
    };

    Ok(estimate)
}

/// Gas allocated to verifying a smart contract signature. Copied from BoundlessMarket.sol.
const ERC1271_MAX_GAS_FOR_CHECK: u64 = 100000;

async fn estimate_gas_to_lock(config: &MarketConfig, order: &OrderRequest) -> anyhow::Result<u64> {
    let mut estimate = config.lockin_gas_estimate;

    if order.request.is_smart_contract_signed() {
        estimate += ERC1271_MAX_GAS_FOR_CHECK;
    }

    Ok(estimate)
}
