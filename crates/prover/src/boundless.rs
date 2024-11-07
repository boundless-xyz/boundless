use std::time::Duration;

use crate::ProofOutput;
use alloy::{
    network::Ethereum,
    primitives::{aliases::U96, utils::parse_ether},
    providers::Provider,
    transports::Transport,
};
use async_trait::async_trait;
use boundless_market::{
    contracts::{Input, Offer, Predicate, ProvingRequest, Requirements},
    sdk::client::Client,
    storage::StorageProvider,
};
use risc0_zkvm::{
    compute_image_id, default_executor,
    sha::{Digest, Digestible},
    ExecutorEnv,
};

use super::AsyncProve;

/// A program uploaded to the configured storage provider of the [Client].
/// 
/// A proof can be generated using the [AsyncProve::prove] method.
#[derive(Clone)]
pub struct BoundlessProgram<T, P, S, F>
where
    F: Fn(u64) -> anyhow::Result<Offer>,
{
    image_url: String,
    client: Client<T, P, S>,

    elf: Vec<u8>,
    image_id: Digest,

    polling_interval: Duration,

    get_offer: F,
}

fn default_offer_from_mcycles(mcycles_count: u64) -> anyhow::Result<Offer> {
    Ok(Offer::default()
        .with_min_price_per_mcycle(
            U96::from::<u128>(parse_ether("0.0001")?.try_into()?),
            mcycles_count,
        )
        .with_max_price_per_mcycle(
            U96::from::<u128>(parse_ether("0.001")?.try_into()?),
            mcycles_count,
        )
        .with_timeout(150))
}

impl<T, P, S, F> BoundlessProgram<T, P, S, F>
where
    F: Fn(u64) -> anyhow::Result<Offer>,
{
    // TODO revisit this API, perhaps not the cleanest to just assume mcycles is only important context
    //      and also this could be async to check a network for what the current price/mcycle is.
    pub fn with_offer_fn<F2>(self, get_offer: F2) -> BoundlessProgram<T, P, S, F2>
    where
        F2: Fn(u64) -> anyhow::Result<Offer>,
    {
        let Self { image_url, client, elf, image_id, polling_interval, get_offer: _ } = self;
        BoundlessProgram { image_url, client, elf, image_id, polling_interval, get_offer }
    }

    /// Update the interval to poll the network for requests to be fulfilled.
    pub fn with_polling_interval(
        mut self,
        polling_interval: Duration,
    ) -> BoundlessProgram<T, P, S, F> {
        self.polling_interval = polling_interval;
        self
    }
}

impl<T, P, S> BoundlessProgram<T, P, S, fn(u64) -> anyhow::Result<Offer>>
where
    T: Transport + Clone + Send + Sync,
    P: Provider<T, Ethereum> + 'static + Clone + Send + Sync,
    S: StorageProvider + Clone + Send + Sync,
{
    pub async fn upload_elf(client: Client<T, P, S>, elf: impl Into<Vec<u8>>) -> anyhow::Result<Self> {
        let elf = elf.into();
        let image_url = client.upload_image(&elf).await?;

        let image_id = compute_image_id(&elf)?;

        Ok(Self::from_parts(image_url, client, elf, image_id))
    }

    pub fn from_parts(
        image_url: String,
        client: Client<T, P, S>,
        elf: Vec<u8>,
        image_id: Digest,
    ) -> Self {
        Self {
            image_url,
            client,
            elf,
            image_id,
            polling_interval: Duration::from_secs(5),
            get_offer: default_offer_from_mcycles,
        }
    }
}

#[async_trait]
impl<T, P, S, F> AsyncProve for BoundlessProgram<T, P, S, F>
where
    T: Transport + Clone + Send + Sync,
    P: Provider<T, Ethereum> + 'static + Clone + Send + Sync,
    S: StorageProvider + Clone + Send + Sync,
    <S as StorageProvider>::Error: std::fmt::Debug,
    F: Fn(u64) -> anyhow::Result<Offer> + Send + Sync,
{
    async fn prove(&self, input: Vec<u8>) -> anyhow::Result<ProofOutput> {
        let elf = self.elf.clone();
        let input_url = self.client.upload_input(&input).await?;

        let session_info = tokio::task::spawn_blocking(move || {
            let env = ExecutorEnv::builder().write_slice(&input).build()?;
            default_executor().execute(env, &elf)
        })
        .await??;
        let mcycles_count = session_info
            .segments
            .iter()
            .map(|segment| 1 << segment.po2)
            .sum::<u64>()
            .div_ceil(1_000_000);
        let journal = session_info.journal;

        let offer = (self.get_offer)(mcycles_count)?;
        let offer_timeout = offer.timeout;
        let request = ProvingRequest::default()
            .with_image_url(&self.image_url)
            .with_input(Input::url(&input_url))
            .with_requirements(Requirements::new(
                self.image_id,
                Predicate::digest_match(journal.digest()),
            ))
            .with_offer(offer);

        // Send the request and wait for it to be completed.
        let request_id = self.client.submit_request(&request).await?;
        tracing::debug!("Request {} submitted", request_id);

        // Wait for the request to be fulfilled by the market, returning the journal and seal.
        let (journal, seal) = tokio::time::timeout(
            Duration::from_secs(offer_timeout as u64 * 12),
            self.client.wait_for_request_fulfillment(request_id, self.polling_interval, None),
        )
        .await??;

        Ok(ProofOutput { journal: journal.into(), seal: seal.into() })
    }
}
