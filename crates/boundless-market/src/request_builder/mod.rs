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

#![allow(missing_docs)] // DO NOT MERGE: That would be too lazy
#![allow(async_fn_in_trait)] // DO NOT MERGE: Consider alternatives.

// TODO: Add debug and trace logging to the layers.
// TODO: Create a test and an example of adding a layer.

use std::{borrow::Cow, fmt, fmt::Debug};

use alloy::{network::Ethereum, providers::Provider};
use derive_builder::Builder;
use risc0_zkvm::Journal;
use url::Url;

use crate::{
    contracts::{Input as RequestInput, Offer, ProofRequest, RequestId, Requirements},
    input::GuestEnv,
    storage::{BuiltinStorageProvider, StorageProvider},
};
mod preflight_layer;
mod storage_layer;

pub use preflight_layer::PreflightLayer;
pub use storage_layer::StorageLayer;
mod requirements_layer;
pub use requirements_layer::RequirementsLayer;
mod request_id_layer;
pub use request_id_layer::{RequestIdLayer, RequestIdLayerMode};
mod offer_layer;
pub use offer_layer::OfferLayer;
mod finalizer;
pub use finalizer::Finalizer;

pub trait RequestBuilder<Params> {
    /// Error type that may be returned by this filler.
    type Error;

    // NOTE: Takes the self receiver so that the caller does not need to explicitly name the
    // RequestBuilder type (e.g. `<MyRequestBuilder as RequestBuilder>::default_params()`)
    fn default_params(&self) -> Params
    where
        Params: Default,
    {
        Default::default()
    }

    async fn build(&self, params: impl Into<Params>) -> Result<ProofRequest, Self::Error>;
}

/// Blanket implementation for [RequestBuilder] for all [Layer] that output a proof request.
///
/// This implementation allows for custom and modified layered builders to automatically be usable
/// as a [RequestBuilder].
impl<L, Params> RequestBuilder<Params> for L
where
    L: Layer<Params, Output = ProofRequest>,
{
    type Error = L::Error;

    async fn build(&self, params: impl Into<Params>) -> Result<ProofRequest, Self::Error> {
        self.process(params.into()).await
    }
}

pub trait Layer<Input> {
    type Output;
    /// Error type that may be returned by this layer.
    type Error;

    async fn process(&self, input: Input) -> Result<Self::Output, Self::Error>;
}

pub trait Adapt<L> {
    type Output;
    type Error;

    async fn process_with(self, layer: &L) -> Result<Self::Output, Self::Error>;
}

impl<L, I> Adapt<L> for I
where
    L: Layer<I>,
{
    type Output = L::Output;
    type Error = L::Error;

    async fn process_with(self, layer: &L) -> Result<Self::Output, Self::Error> {
        layer.process(self).await
    }
}

/// Define a layer as a stack of two layers. Output of layer A is piped into layer B.
impl<A, B, Input> Layer<Input> for (A, B)
where
    Input: Adapt<A>,
    <Input as Adapt<A>>::Output: Adapt<B>,
    <Input as Adapt<A>>::Error: Into<<<Input as Adapt<A>>::Output as Adapt<B>>::Error>,
{
    type Output = <<Input as Adapt<A>>::Output as Adapt<B>>::Output;
    type Error = <<Input as Adapt<A>>::Output as Adapt<B>>::Error;

    async fn process(&self, input: Input) -> Result<Self::Output, Self::Error> {
        input.process_with(&self.0).await.map_err(Into::into)?.process_with(&self.1).await
    }
}

/// A standard [RequestBuilder] provided as a default implementation.
#[derive(Clone, Builder)]
#[non_exhaustive]
pub struct StandardRequestBuilder<P, S = BuiltinStorageProvider> {
    #[builder(setter(into))]
    pub storage_layer: StorageLayer<S>,
    #[builder(setter(into), default)]
    pub preflight_layer: PreflightLayer,
    #[builder(setter(into), default)]
    pub requirements_layer: RequirementsLayer,
    #[builder(setter(into))]
    pub request_id_layer: RequestIdLayer<P>,
    #[builder(setter(into))]
    pub offer_layer: OfferLayer<P>,
    #[builder(setter(into), default)]
    pub finalizer: Finalizer,
}

impl StandardRequestBuilder<(), ()> {
    pub fn builder<P: Clone, S: Clone>() -> StandardRequestBuilderBuilder<P, S> {
        Default::default()
    }
}

impl<P, S> Layer<RequestParams> for StandardRequestBuilder<P, S>
where
    S: StorageProvider, // DO NOT MERGE: handle cases where the storage_provider is not provided.
    S::Error: std::error::Error + Send + Sync + 'static,
    P: Provider<Ethereum> + 'static + Clone,
{
    type Output = ProofRequest;
    type Error = anyhow::Error;

    async fn process(&self, input: RequestParams) -> Result<ProofRequest, Self::Error> {
        input
            .process_with(&self.storage_layer)
            .await?
            .process_with(&self.preflight_layer)
            .await?
            .process_with(&self.requirements_layer)
            .await?
            .process_with(&self.request_id_layer)
            .await?
            .process_with(&self.offer_layer)
            .await?
            .process_with(&self.finalizer)
            .await
    }
}

// NOTE: We don't use derive_builder here because we need to be able to access the values on the
// incrementally built parameters.
#[non_exhaustive]
#[derive(Clone, Default)]
pub struct RequestParams {
    /// RISC-V guest program that will be run in the zkVM.
    pub program: Option<Cow<'static, [u8]>>,
    /// Guest execution environment, providing the input for the guest.
    /// See [GuestEnv].
    pub env: Option<GuestEnv>,
    /// Uploaded program URL, from which provers will fetch the program.
    pub program_url: Option<Url>,
    /// Prepared input for the [ProofRequest], containing either a URL or inline input.
    /// See [RequestInput].
    pub request_input: Option<RequestInput>,
    /// Count of the RISC Zero execution cycles. Used to estimate proving cost.
    pub cycles: Option<u64>,
    /// Contents of the [Journal] that results from the execution.
    pub journal: Option<Journal>,
    /// [RequestId] to use for the proof request.
    pub request_id: Option<RequestId>,
    /// [Offer] to send along with the request.
    pub offer: Option<Offer>,
    /// [Requirements] for the resulting proof.
    pub requirements: Option<Requirements>,
}

impl RequestParams {
    pub fn require_program(&self) -> Result<&[u8], MissingFieldError> {
        self.program.as_deref().ok_or(MissingFieldError::new("program"))
    }

    pub fn with_program(self, value: impl Into<Cow<'static, [u8]>>) -> Self {
        Self { program: Some(value.into()), ..self }
    }

    pub fn require_env(&self) -> Result<&GuestEnv, MissingFieldError> {
        self.env.as_ref().ok_or(MissingFieldError::new("env"))
    }

    pub fn with_env(self, value: impl Into<GuestEnv>) -> Self {
        Self { env: Some(value.into()), ..self }
    }

    pub fn require_program_url(&self) -> Result<&Url, MissingFieldError> {
        self.program_url.as_ref().ok_or(MissingFieldError::new("program_url"))
    }

    pub fn with_program_url(self, value: impl Into<Url>) -> Self {
        Self { program_url: Some(value.into()), ..self }
    }

    pub fn require_request_input(&self) -> Result<&RequestInput, MissingFieldError> {
        self.request_input.as_ref().ok_or(MissingFieldError::new("input"))
    }

    pub fn with_request_input(self, value: impl Into<RequestInput>) -> Self {
        Self { request_input: Some(value.into()), ..self }
    }

    pub fn require_cycles(&self) -> Result<u64, MissingFieldError> {
        self.cycles.ok_or(MissingFieldError::new("cycles"))
    }

    pub fn with_cycles(self, value: u64) -> Self {
        Self { cycles: Some(value), ..self }
    }

    pub fn require_journal(&self) -> Result<&Journal, MissingFieldError> {
        self.journal.as_ref().ok_or(MissingFieldError::new("journal"))
    }

    pub fn with_journal(self, value: impl Into<Journal>) -> Self {
        Self { journal: Some(value.into()), ..self }
    }

    pub fn require_request_id(&self) -> Result<&RequestId, MissingFieldError> {
        self.request_id.as_ref().ok_or(MissingFieldError::new("request_id"))
    }

    pub fn with_request_id(self, value: impl Into<RequestId>) -> Self {
        Self { request_id: Some(value.into()), ..self }
    }

    pub fn require_offer(&self) -> Result<&Offer, MissingFieldError> {
        self.offer.as_ref().ok_or(MissingFieldError::new("offer"))
    }

    pub fn with_offer(self, value: impl Into<Offer>) -> Self {
        Self { offer: Some(value.into()), ..self }
    }

    pub fn require_requirements(&self) -> Result<&Requirements, MissingFieldError> {
        self.requirements.as_ref().ok_or(MissingFieldError::new("requirements"))
    }

    pub fn with_requirements(self, value: impl Into<Requirements>) -> Self {
        Self { requirements: Some(value.into()), ..self }
    }
}

impl Debug for RequestParams {
    /// [Debug] implementation that does not print the contents of the program.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ExampleRequestParams")
            .field("program", &self.program.as_ref().map(|x| format!("[{} bytes]", x.len())))
            .field("env", &self.env)
            .field("program_url", &self.program_url)
            .field("input", &self.request_input)
            .field("cycles", &self.cycles)
            .field("journal", &self.journal)
            .field("request_id", &self.request_id)
            .field("offer", &self.offer)
            .field("requirements", &self.requirements)
            .finish()
    }
}

impl<Program, Env> From<(Program, Env)> for RequestParams
where
    Program: Into<Cow<'static, [u8]>>,
    Env: Into<GuestEnv>,
{
    fn from(value: (Program, Env)) -> Self {
        Self::default().with_program(value.0).with_env(value.1)
    }
}

#[derive(thiserror::Error, Debug)]
#[error("field `{label}` is required but is uninitialized")]
pub struct MissingFieldError {
    pub label: Cow<'static, str>,
}

impl MissingFieldError {
    pub fn new(label: impl Into<Cow<'static, str>>) -> Self {
        Self { label: label.into() }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use alloy::node_bindings::Anvil;
    use boundless_market_test_utils::{create_test_ctx, ECHO_ELF};
    use tracing_test::traced_test;

    use super::{
        Layer, OfferLayer, PreflightLayer, RequestBuilder, RequestId, RequestIdLayer,
        RequestIdLayerMode, RequirementsLayer, StandardRequestBuilder, StorageLayer,
    };

    use crate::{
        contracts::{
            boundless_market::BoundlessMarketService, Input as RequestInput, InputType, Predicate,
            Requirements,
        },
        input::GuestEnv,
        storage::{fetch_url, MockStorageProvider, StorageProvider},
    };
    use alloy_primitives::U256;
    use risc0_zkvm::{compute_image_id, sha::Digestible, Journal};

    #[tokio::test]
    #[traced_test]
    async fn basic() -> anyhow::Result<()> {
        let anvil = Anvil::new().spawn();
        let test_ctx = create_test_ctx(&anvil).await.unwrap();
        let storage = Arc::new(MockStorageProvider::start());
        let market = BoundlessMarketService::new(
            test_ctx.boundless_market_address,
            test_ctx.customer_provider.clone(),
            test_ctx.customer_signer.address(),
        );

        let request_builder = StandardRequestBuilder::builder()
            .storage_layer(storage)
            .offer_layer(test_ctx.customer_provider.clone())
            .request_id_layer(market)
            .build()?;

        let params =
            request_builder.default_params().with_program(ECHO_ELF).with_env(b"hello!".to_vec());
        let request = request_builder.build(params).await?;
        println!("built request {request:#?}");
        Ok(())
    }

    #[tokio::test]
    #[traced_test]
    async fn test_storage_layer() -> anyhow::Result<()> {
        let storage = Arc::new(MockStorageProvider::start());
        let layer = StorageLayer::builder()
            .storage_provider(storage.clone())
            .inline_input_max_bytes(Some(1024))
            .build()?;
        let env = GuestEnv::from(b"inline_data".to_vec());
        let (program_url, request_input) = layer.process((ECHO_ELF, &env)).await?;

        // Program should be uploaded and input inline.
        assert_eq!(fetch_url(&program_url).await?, ECHO_ELF);
        assert_eq!(request_input.inputType, InputType::Inline);
        assert_eq!(request_input.data, env.encode()?);
        Ok(())
    }

    #[tokio::test]
    #[traced_test]
    async fn test_storage_layer_large_input() -> anyhow::Result<()> {
        let storage = Arc::new(MockStorageProvider::start());
        let layer = StorageLayer::builder()
            .storage_provider(storage.clone())
            .inline_input_max_bytes(Some(1024))
            .build()?;
        let env = GuestEnv::from(rand::random_iter().take(2048).collect::<Vec<u8>>());
        let (program_url, request_input) = layer.process((ECHO_ELF, &env)).await?;

        // Program and input should be uploaded and input inline.
        assert_eq!(fetch_url(&program_url).await?, ECHO_ELF);
        assert_eq!(request_input.inputType, InputType::Url);
        let fetched_input = fetch_url(String::from_utf8(request_input.data.to_vec())?).await?;
        assert_eq!(fetched_input, env.encode()?);
        Ok(())
    }

    #[tokio::test]
    #[traced_test]
    async fn test_preflight_layer() -> anyhow::Result<()> {
        let storage = MockStorageProvider::start();
        let program_url = storage.upload_program(ECHO_ELF).await?;
        let layer = PreflightLayer::default();
        let data = b"hello_zkvm".to_vec();
        let env = GuestEnv::from(data.clone());
        let input = RequestInput::inline(env.encode()?);
        let session = layer.process((&program_url, &input)).await?;

        assert_eq!(session.journal.as_ref(), data.as_slice());
        // Verify non-zero cycle count and an exit code of zero.
        let cycles: u64 = session.segments.iter().map(|s| 1 << s.po2).sum();
        assert!(cycles > 0);
        assert!(session.exit_code.is_ok());
        Ok(())
    }

    #[tokio::test]
    #[traced_test]
    async fn test_requirements_layer() -> anyhow::Result<()> {
        let layer = RequirementsLayer::default();
        let program = ECHO_ELF;
        let bytes = b"journal_data".to_vec();
        let journal = Journal::new(bytes.clone());
        let req = layer.process((program, &journal)).await?;

        // Predicate should match the same journal
        assert!(req.predicate.eval(&journal));
        // And should not match different data
        let other = Journal::new(b"other_data".to_vec());
        assert!(!req.predicate.eval(&other));
        Ok(())
    }

    #[tokio::test]
    #[traced_test]
    async fn test_request_id_layer_rand() -> anyhow::Result<()> {
        let anvil = Anvil::new().spawn();
        let test_ctx = create_test_ctx(&anvil).await?;
        let market = BoundlessMarketService::new(
            test_ctx.boundless_market_address,
            test_ctx.customer_provider.clone(),
            test_ctx.customer_signer.address(),
        );
        let layer = RequestIdLayer::from(market.clone());
        assert_eq!(layer.mode, RequestIdLayerMode::Rand);
        let id = layer.process(()).await?;
        assert_eq!(id.addr, test_ctx.customer_signer.address());
        assert!(!id.smart_contract_signed);
        Ok(())
    }

    #[tokio::test]
    #[traced_test]
    async fn test_request_id_layer_nonce() -> anyhow::Result<()> {
        let anvil = Anvil::new().spawn();
        let test_ctx = create_test_ctx(&anvil).await?;
        let market = BoundlessMarketService::new(
            test_ctx.boundless_market_address,
            test_ctx.customer_provider.clone(),
            test_ctx.customer_signer.address(),
        );
        let layer = RequestIdLayer::builder()
            .boundless_market(market.clone())
            .mode(RequestIdLayerMode::Nonce)
            .build()?;

        let id = layer.process(()).await?;
        assert_eq!(id.addr, test_ctx.customer_signer.address());
        // The customer address has sent no transactions.
        assert_eq!(id.index, 0);
        assert!(!id.smart_contract_signed);
        // TODO: Send a tx then check that the index increments.
        Ok(())
    }

    // OfferLayer test
    #[tokio::test]
    #[traced_test]
    async fn test_offer_layer_estimates() -> anyhow::Result<()> {
        // Use Anvil-backed provider for gas price
        let anvil = Anvil::new().spawn();
        let test_ctx = create_test_ctx(&anvil).await?;
        let provider = test_ctx.customer_provider.clone();
        let layer = OfferLayer::from(provider.clone());
        // Build minimal requirements and request ID
        let image_id = compute_image_id(ECHO_ELF).unwrap();
        let predicate = Predicate::digest_match(Journal::new(b"hello".to_vec()).digest());
        let requirements = Requirements::new(image_id, predicate);
        let request_id = RequestId::new(test_ctx.customer_signer.address(), 0);

        // Zero cycles
        let offer_zero_mcycles = layer.process((&requirements, &request_id, 0u64)).await?;
        assert_eq!(offer_zero_mcycles.minPrice, U256::ZERO);
        // Defaults from builder
        assert_eq!(offer_zero_mcycles.rampUpPeriod, 120);
        assert_eq!(offer_zero_mcycles.lockTimeout, 600);
        assert_eq!(offer_zero_mcycles.timeout, 1200);
        // Max price should be non-negative, to account for fixed costs.
        assert!(offer_zero_mcycles.maxPrice > U256::ZERO);

        // Now create an offer for 100 Mcycles.
        let offer_more_mcycles = layer.process((&requirements, &request_id, 100u64 << 20)).await?;
        assert!(offer_more_mcycles.maxPrice > offer_zero_mcycles.maxPrice);
        Ok(())
    }
}
