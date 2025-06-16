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

use super::{Adapt, Layer, RequestParams};
use crate::{contracts::boundless_market::BoundlessMarketService, contracts::RequestId};
use alloy::{network::Ethereum, providers::Provider};
use derive_builder::Builder;

/// Mode for generating request IDs.
///
/// Controls how the request ID's index is generated.
#[non_exhaustive]
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum RequestIdLayerMode {
    /// Generate a random ID for each request.
    ///
    /// This is the default mode and generates unpredictable request IDs.
    #[default]
    Rand,

    /// Use the account's nonce as the ID.
    ///
    /// This creates sequential IDs based on the number of transactions sent from the account.
    Nonce,
}

/// Configuration for the [RequestIdLayer].
///
/// Controls how request IDs are generated.
#[non_exhaustive]
#[derive(Clone, Builder)]
pub struct RequestIdLayerConfig {
    /// The mode to use for generating request IDs.
    #[builder(default)]
    pub mode: RequestIdLayerMode,
}

/// A layer responsible for generating request IDs.
///
/// This layer creates unique identifiers for proof requests based on the
/// configured generation mode.
#[non_exhaustive]
#[derive(Clone)]
pub struct RequestIdLayer<P> {
    /// The BoundlessMarket service used to generate request IDs.
    pub boundless_market: BoundlessMarketService<P>,

    /// Configuration controlling ID generation behavior.
    pub config: RequestIdLayerConfig,
}

impl RequestIdLayerConfig {
    /// Creates a new builder for constructing a [RequestIdLayerConfig].
    ///
    /// This provides a way to customize the request ID generation behavior.
    pub fn builder() -> RequestIdLayerConfigBuilder {
        Default::default()
    }
}

impl<P: Clone> From<BoundlessMarketService<P>> for RequestIdLayer<P> {
    fn from(boundless_market: BoundlessMarketService<P>) -> Self {
        RequestIdLayer { boundless_market, config: Default::default() }
    }
}

impl Default for RequestIdLayerConfig {
    fn default() -> Self {
        Self::builder().build().expect("implementation error in Default for RequestIdLayerConfig")
    }
}

impl<P> RequestIdLayer<P> {
    /// Creates a new [RequestIdLayer] with the specified market service and configuration.
    ///
    /// The BoundlessMarket service is used to generate request IDs according to the
    /// configured mode.
    pub fn new(boundless_market: BoundlessMarketService<P>, config: RequestIdLayerConfig) -> Self {
        Self { boundless_market, config }
    }
}

impl<P> Layer<()> for RequestIdLayer<P>
where
    P: Provider<Ethereum> + 'static + Clone,
{
    type Output = RequestId;
    type Error = anyhow::Error;

    async fn process(&self, (): ()) -> Result<Self::Output, Self::Error> {
        let id_u256 = match self.config.mode {
            RequestIdLayerMode::Nonce => self.boundless_market.request_id_from_nonce().await?,
            RequestIdLayerMode::Rand => self.boundless_market.request_id_from_rand().await?,
        };
        Ok(id_u256.try_into().expect("generated request ID should always be valid"))
    }
}

impl<P> Adapt<RequestIdLayer<P>> for RequestParams
where
    P: Provider<Ethereum> + 'static + Clone,
{
    type Output = RequestParams;
    type Error = anyhow::Error;

    async fn process_with(self, layer: &RequestIdLayer<P>) -> Result<Self::Output, Self::Error> {
        tracing::trace!("Processing {self:?} with RequestIdLayer");

        if self.request_id.is_some() {
            return Ok(self);
        }

        let request_id = layer.process(()).await?;
        Ok(self.with_request_id(request_id))
    }
}
