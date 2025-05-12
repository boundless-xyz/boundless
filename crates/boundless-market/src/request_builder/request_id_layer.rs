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
use crate::contracts::boundless_market::BoundlessMarketService;
use crate::contracts::RequestId;
use alloy::{network::Ethereum, providers::Provider};
use derive_builder::Builder;

#[non_exhaustive]
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum RequestIdLayerMode {
    #[default]
    Rand,
    Nonce,
}

#[non_exhaustive]
#[derive(Clone, Builder)]
pub struct RequestIdLayer<P> {
    pub boundless_market: BoundlessMarketService<P>,
    #[builder(default)]
    pub mode: RequestIdLayerMode,
}

impl RequestIdLayer<()> {
    pub fn builder<P: Clone>() -> RequestIdLayerBuilder<P> {
        Default::default()
    }
}

impl<P: Clone> From<BoundlessMarketService<P>> for RequestIdLayer<P> {
    fn from(value: BoundlessMarketService<P>) -> Self {
        RequestIdLayer::builder()
            .boundless_market(value)
            .build()
            .expect("implementation error in From<BoundlessMarketService<P>> for RequestIdLayer")
    }
}

impl<P> Layer<()> for RequestIdLayer<P>
where
    P: Provider<Ethereum> + 'static + Clone,
{
    type Output = RequestId;
    type Error = anyhow::Error;

    async fn process(&self, (): ()) -> Result<Self::Output, Self::Error> {
        let id_u256 = match self.mode {
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
