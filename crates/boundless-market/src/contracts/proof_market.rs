// Copyright (c) 2024 RISC Zero, Inc.
//
// All rights reserved.

use std::time::Duration;

use alloy::{
    network::Ethereum,
    primitives::{aliases::U192, Address, Bytes, B256, U256},
    providers::Provider,
    signers::{Signer, SignerSync},
    transports::Transport,
};
use alloy_sol_types::SolEvent;
use anyhow::{anyhow, Context, Result};
use thiserror::Error;

use super::{
    request_id, Fulfillment,
    IProofMarket::{self, IProofMarketErrors, IProofMarketInstance},
    Offer, ProofStatus, ProvingRequest, TxnErr, TXN_CONFIRM_TIMEOUT,
};

/// Proof market errors.
#[derive(Error, Debug)]
pub enum MarketError {
    #[error("Transaction error: {0}")]
    TxnError(#[from] TxnErr),

    #[error("Request is not fulfilled {0}")]
    RequestNotFulfilled(U256),

    #[error("Request has expired {0}")]
    RequestHasExpired(U256),

    #[error("Proof not found {0}")]
    ProofNotFound(U256),

    #[error("Lockin reverted, possibly outbid: txn_hash: {0}")]
    LockRevert(B256),

    #[error("Market error: {0}")]
    Error(#[from] anyhow::Error),

    #[error("Timeout: {0}")]
    TimeoutReached(U256),
}

impl From<alloy::contract::Error> for MarketError {
    fn from(err: alloy::contract::Error) -> Self {
        MarketError::Error(err.into())
    }
}

/// Proof market service.
#[derive(Clone)]
pub struct ProofMarketService<T, P> {
    instance: IProofMarketInstance<T, P, Ethereum>,
    caller: Address,
    timeout: Duration,
    event_query_config: EventQueryConfig,
}
/// Event query configuration.
#[derive(Clone)]
#[non_exhaustive]
pub struct EventQueryConfig {
    /// Maximum number of iterations to search for a fulfilled event.
    pub max_iterations: u64,
    /// Number of blocks to query in each iteration when searching for a fulfilled event.
    pub block_range: u64,
}

impl Default for EventQueryConfig {
    fn default() -> Self {
        Self { max_iterations: 100, block_range: 100 }
    }
}

impl EventQueryConfig {
    /// Creates a new event query configuration.
    pub fn new(max_iterations: u64, block_range: u64) -> Self {
        Self { max_iterations, block_range }
    }

    /// Sets the maximum number of iterations to search for a fulfilled event.
    pub fn with_max_iterations(self, max_iterations: u64) -> Self {
        Self { max_iterations, ..self }
    }

    /// Sets the number of blocks to query in each iteration when searching for a fulfilled event.
    pub fn with_block_range(self, block_range: u64) -> Self {
        Self { block_range, ..self }
    }
}

impl<T, P> ProofMarketService<T, P>
where
    T: Transport + Clone,
    P: Provider<T, Ethereum> + 'static + Clone,
{
    /// Creates a new proof market service.
    pub fn new(address: Address, provider: P, caller: Address) -> Self {
        let instance = IProofMarket::new(address, provider);

        Self {
            instance,
            caller,
            timeout: TXN_CONFIRM_TIMEOUT,
            event_query_config: EventQueryConfig::default(),
        }
    }

    /// Sets the transaction timeout.
    pub fn with_timeout(self, timeout: Duration) -> Self {
        Self { timeout, ..self }
    }

    /// Sets the event query configuration.
    pub fn with_event_query_config(self, config: EventQueryConfig) -> Self {
        Self { event_query_config: config, ..self }
    }

    /// Returns the proof market instance.
    pub fn instance(&self) -> &IProofMarketInstance<T, P, Ethereum> {
        &self.instance
    }

    /// Returns the caller address.
    pub fn caller(&self) -> Address {
        self.caller
    }

    /// Deposit Ether into the proof market to pay for proof and/or lockin stake.
    pub async fn deposit(&self, value: U256) -> Result<(), MarketError> {
        tracing::debug!("Calling deposit() value: {value}");
        let call = self.instance.deposit().value(value);
        let pending_tx = call.send().await.map_err(IProofMarketErrors::decode_error)?;
        tracing::debug!("Broadcasting deposit tx {}", pending_tx.tx_hash());
        let tx_hash = pending_tx
            .with_timeout(Some(self.timeout))
            .watch()
            .await
            .context("failed to confirm tx")?;
        tracing::debug!("Submitted deposit {}", tx_hash);

        Ok(())
    }

    /// Withdraw Ether from the proof market.
    pub async fn withdraw(&self, amount: U256) -> Result<(), MarketError> {
        tracing::debug!("Calling withdraw({amount})");
        let call = self.instance.withdraw(amount);
        let pending_tx = call.send().await.map_err(IProofMarketErrors::decode_error)?;
        tracing::debug!("Broadcasting withdraw tx {}", pending_tx.tx_hash());
        let tx_hash = pending_tx
            .with_timeout(Some(self.timeout))
            .watch()
            .await
            .context("failed to confirm tx")?;
        tracing::debug!("Submitted withdraw {}", tx_hash);

        Ok(())
    }

    /// Returns the balance, in Ether, of the given account.
    pub async fn balance_of(&self, account: Address) -> Result<U256, MarketError> {
        tracing::debug!("Calling balanceOf({account})");
        let balance = self
            .instance
            .balanceOf(account)
            .call()
            .await
            .map_err(IProofMarketErrors::decode_error)?
            ._0;

        Ok(balance)
    }

    /// Submit a proving request such that it is publicly available for provers to evaluate and bid
    /// on.
    pub async fn submit_request(
        &self,
        request: &ProvingRequest,
        signer: &(impl Signer + SignerSync),
    ) -> Result<U256, MarketError> {
        tracing::debug!("Calling submitRequest({:?})", request);
        let provider = self.instance.provider();
        let chain_id = provider.get_chain_id().await.context("Failed to get chain ID")?;
        let client_sig = request
            .sign_request(signer, *self.instance.address(), chain_id)
            .context("Failed to sign proving request")?;
        let call = self
            .instance
            .submitRequest(request.clone(), client_sig.as_bytes().into())
            .from(self.caller)
            .value(U256::from(request.offer.maxPrice));
        let pending_tx = call.send().await.map_err(IProofMarketErrors::decode_error)?;
        tracing::debug!("Broadcasting tx {}", pending_tx.tx_hash());

        let receipt = pending_tx
            .with_timeout(Some(self.timeout))
            .get_receipt()
            .await
            .context("failed to confirm tx")?;
        let [log] = receipt.inner.logs() else {
            return Err(MarketError::Error(anyhow!("call must emit exactly one event")));
        };
        let log = log.log_decode::<IProofMarket::RequestSubmitted>().with_context(|| {
            format!("call did not emit {}", IProofMarket::RequestSubmitted::SIGNATURE)
        })?;

        Ok(U256::from(log.inner.data.request.id))
    }

    /// Lock the proving request to the prover, giving them exclusive rights to be paid to
    /// fulfill this request, and also making them subject to slashing penalties if they fail to
    /// deliver. At this point, the price for fulfillment is also set, based on the reverse Dutch
    /// auction parameters and the block at which this transaction is processed.
    ///
    /// This method should be called from the address of the prover.
    pub async fn lockin_request(
        &self,
        request: &ProvingRequest,
        client_sig: &Bytes,
        priority_gas: Option<u64>,
    ) -> Result<u64, MarketError> {
        tracing::debug!("Calling requestIsLocked({:x})", request.id);
        let is_locked_in: bool =
            self.instance.requestIsLocked(request.id).call().await.context("call failed")?._0;
        if is_locked_in {
            return Err(MarketError::Error(anyhow!("request is already locked-in")));
        }

        tracing::debug!("Calling lockin({:?}, {:?})", request, client_sig);

        let mut call = self.instance.lockin(request.clone(), client_sig.clone()).from(self.caller);

        if let Some(gas) = priority_gas {
            let priority_fee = self
                .instance
                .provider()
                .estimate_eip1559_fees(None)
                .await
                .context("Failed to get priority gas fee")?;

            call = call
                .max_fee_per_gas(priority_fee.max_fee_per_gas + gas as u128)
                .max_priority_fee_per_gas(priority_fee.max_priority_fee_per_gas + gas as u128);
        }

        let pending_tx = call.send().await.map_err(IProofMarketErrors::decode_error)?;

        tracing::debug!("Broadcasting tx {}", pending_tx.tx_hash());

        let receipt = pending_tx
            .with_timeout(Some(self.timeout))
            .get_receipt()
            .await
            .context("failed to confirm tx")?;

        if !receipt.status() {
            // TODO: Get + print revertReason
            return Err(MarketError::LockRevert(receipt.transaction_hash));
        }

        tracing::info!("Registered request {:x}: {}", request.id, receipt.transaction_hash);

        Ok(receipt.block_number.context("TXN Receipt missing block number")?)
    }

    /// Lock the proving request to the prover, giving them exclusive rights to be paid to
    /// fulfill this request, and also making them subject to slashing penalties if they fail to
    /// deliver. At this point, the price for fulfillment is also set, based on the reverse Dutch
    /// auction parameters and the block at which this transaction is processed.
    ///
    /// This method uses the provided signature to authenticate the prover.
    pub async fn lockin_request_with_sig(
        &self,
        request: &ProvingRequest,
        client_sig: &Bytes,
        prover_sig: &Bytes,
        _priority_gas: Option<u128>,
    ) -> Result<u64, MarketError> {
        tracing::debug!("Calling requestIsLocked({:x})", request.id);
        let is_locked_in: bool =
            self.instance.requestIsLocked(request.id).call().await.context("call failed")?._0;
        if is_locked_in {
            return Err(MarketError::Error(anyhow!("request is already locked-in")));
        }

        tracing::debug!("Calling lockinWithSig({:?}, {:?}, {:?})", request, client_sig, prover_sig);

        let call = self
            .instance
            .lockinWithSig(request.clone(), client_sig.clone(), prover_sig.clone())
            .from(self.caller);
        let pending_tx = call.send().await.context("Failed to lock")?;

        tracing::debug!("Broadcasting tx {}", pending_tx.tx_hash());

        let receipt = pending_tx
            .with_timeout(Some(self.timeout))
            .get_receipt()
            .await
            .context("failed to confirm tx")?;

        if !receipt.status() {
            // TODO: Get + print revertReason
            return Err(MarketError::Error(anyhow!(
                "LockinRequest failed [{}], possibly outbid",
                receipt.transaction_hash
            )));
        }

        tracing::info!("Registered request {:x}: {}", request.id, receipt.transaction_hash);

        Ok(receipt.block_number.context("TXN Receipt missing block number")?)
    }

    /// When a prover fails to fulfill a request by the deadline, this function can be used to burn
    /// the associated prover stake.
    pub async fn slash(&self, request_id: U256) -> Result<U256, MarketError> {
        tracing::debug!("Calling slash({:?})", request_id);
        let call = self.instance.slash(U192::from(request_id)).from(self.caller);
        let pending_tx = call.send().await.map_err(IProofMarketErrors::decode_error)?;
        tracing::debug!("Broadcasting tx {}", pending_tx.tx_hash());

        let receipt = pending_tx
            .with_timeout(Some(self.timeout))
            .get_receipt()
            .await
            .context("failed to confirm tx")?;
        let [log] = receipt.inner.logs() else {
            return Err(MarketError::Error(anyhow!("call must emit exactly one event")));
        };
        let log = log.log_decode::<IProofMarket::LockinStakeBurned>().with_context(|| {
            format!("call did not emit {}", IProofMarket::LockinStakeBurned::SIGNATURE)
        })?;

        Ok(U256::from(log.inner.data.stake))
    }

    /// Fulfill a locked request by delivering the proof for the application.
    /// Upon proof verification, the prover will be paid.
    pub async fn fulfill(
        &self,
        fulfillment: &Fulfillment,
        market_seal: &Bytes,
    ) -> Result<(), MarketError> {
        tracing::debug!("Calling fulfill({:?},{:?})", fulfillment, market_seal);
        let call =
            self.instance.fulfill(fulfillment.clone(), market_seal.clone()).from(self.caller);
        let pending_tx = call.send().await.map_err(IProofMarketErrors::decode_error)?;
        tracing::debug!("Broadcasting tx {}", pending_tx.tx_hash());

        let tx_hash = pending_tx
            .with_timeout(Some(self.timeout))
            .watch()
            .await
            .context("failed to confirm tx")?;

        tracing::info!("Submitted proof for request {}: {}", fulfillment.id, tx_hash);

        Ok(())
    }

    /// Fulfill a batch of locked requests.
    /// Upon proof verification, the prover will be paid.
    pub async fn fulfill_batch(
        &self,
        fulfillments: Vec<Fulfillment>,
        assessor_seal: Bytes,
    ) -> Result<(), MarketError> {
        tracing::debug!("Calling fulfillBatch({fulfillments:?}, {assessor_seal:x})");
        let call = self.instance.fulfillBatch(fulfillments, assessor_seal).from(self.caller);
        tracing::debug!("Calldata: {}", call.calldata());
        let pending_tx = call.send().await.map_err(IProofMarketErrors::decode_error)?;
        tracing::debug!("Broadcasting tx {}", pending_tx.tx_hash());

        let tx_hash = pending_tx
            .with_timeout(Some(self.timeout))
            .watch()
            .await
            .context("failed to confirm tx")?;

        tracing::info!("Submitted proof for batch {}", tx_hash);

        Ok(())
    }

    pub async fn submit_merkle_and_fulfill(
        &self,
        root: B256,
        seal: Bytes,
        fulfillments: Vec<Fulfillment>,
        assessor_seal: Bytes,
    ) -> Result<(), MarketError> {
        tracing::debug!("Calling submitRootAndFulfillBatch({root:?}, {seal:x}, {fulfillments:?}, {assessor_seal:x})");
        let call = self
            .instance
            .submitRootAndFulfillBatch(root, seal, fulfillments, assessor_seal)
            .from(self.caller);
        tracing::debug!("Calldata: {}", call.calldata());
        let pending_tx = call.send().await.map_err(IProofMarketErrors::decode_error)?;
        tracing::debug!("Broadcasting tx {}", pending_tx.tx_hash());
        let tx_hash = pending_tx
            .with_timeout(Some(self.timeout))
            .watch()
            .await
            .context("failed to confirm tx")?;

        tracing::info!("Submitted merkle root and proof for batch {}", tx_hash);

        Ok(())
    }

    /// Checks if a request is locked in.
    pub async fn is_locked_in(&self, request_id: U256) -> Result<bool, MarketError> {
        tracing::debug!("Calling requestIsLocked({})", request_id);
        let res = self
            .instance
            .requestIsLocked(U192::from(request_id))
            .call()
            .await
            .map_err(IProofMarketErrors::decode_error)?;

        Ok(res._0)
    }

    /// Checks if a request is fulfilled.
    pub async fn is_fulfilled(&self, request_id: U256) -> Result<bool, MarketError> {
        tracing::debug!("Calling requestIsFulfilled({})", request_id);
        let res = self
            .instance
            .requestIsFulfilled(U192::from(request_id))
            .call()
            .await
            .map_err(IProofMarketErrors::decode_error)?;

        Ok(res._0)
    }

    /// Returns the [ProofStatus] of a proving request.
    ///
    /// The `expires_at` parameter is the block number at which the request expires.
    pub async fn get_status(
        &self,
        request_id: U256,
        expires_at: Option<u64>,
    ) -> Result<ProofStatus, MarketError> {
        let block_number = self.get_latest_block().await?;

        if self.is_fulfilled(request_id).await.context("Failed to check fulfillment status")? {
            return Ok(ProofStatus::Fulfilled);
        }

        if let Some(expires_at) = expires_at {
            if block_number > expires_at {
                return Ok(ProofStatus::Expired);
            }
        }

        if self.is_locked_in(request_id).await.context("Failed to check locked status")? {
            let deadline = self.instance.requestDeadline(U192::from(request_id)).call().await?._0;
            if block_number > deadline && deadline > 0 {
                return Ok(ProofStatus::Expired);
            };
            return Ok(ProofStatus::Locked);
        }

        Ok(ProofStatus::Unknown)
    }

    async fn get_latest_block(&self) -> Result<u64, MarketError> {
        Ok(self
            .instance
            .provider()
            .get_block_number()
            .await
            .context("Failed to get latest block number")?)
    }

    /// Query the RequestFulfilled event based on request ID and block options.
    /// For each iteration, we query a range of blocks.
    /// If the event is not found, we move the range down and repeat until we find the event.
    /// If the event is not found after the configured max iterations, we return an error.
    /// The default range is set to 100 blocks for each iteration, and the default maximum number of
    /// iterations is 100. This means that the search will cover a maximum of 10,000 blocks.
    /// Optionally, you can specify a lower and upper bound to limit the search range.
    async fn query_fulfilled_event(
        &self,
        request_id: U256,
        lower_bound: Option<u64>,
        upper_bound: Option<u64>,
    ) -> Result<(Bytes, Bytes), MarketError> {
        let mut upper_block = upper_bound.unwrap_or(self.get_latest_block().await?);
        let start_block = lower_bound.unwrap_or(upper_block.saturating_sub(
            self.event_query_config.block_range * self.event_query_config.max_iterations,
        ));

        // Loop to progressively search through blocks
        for _ in 0..self.event_query_config.max_iterations {
            // If the current end block is less than or equal to the starting block, stop searching
            if upper_block <= start_block {
                break;
            }

            // Calculate the block range to query: from [lower_block] to [upper_block]
            let lower_block = upper_block.saturating_sub(self.event_query_config.block_range);

            // Set up the event filter for the specified block range
            let mut event_filter = self.instance.RequestFulfilled_filter();
            event_filter.filter = event_filter
                .filter
                .topic1(request_id)
                .from_block(lower_block)
                .to_block(upper_block);

            // Query the logs for the event
            let logs = event_filter.query().await?;

            // If we find a log, return the journal and seal
            if let Some((log, _)) = logs.first() {
                return Ok((log.journal.clone(), log.seal.clone()));
            }

            // Move the upper_block down for the next iteration
            upper_block = lower_block.saturating_sub(1);
        }

        // Return error if no logs are found after all iterations
        Err(MarketError::ProofNotFound(request_id))
    }

    /// Returns journal and seal if the request is fulfilled.
    pub async fn get_request_fulfillment(
        &self,
        request_id: U256,
    ) -> Result<(Bytes, Bytes), MarketError> {
        match self.get_status(request_id, None).await? {
            ProofStatus::Expired => Err(MarketError::RequestHasExpired(request_id)),
            ProofStatus::Fulfilled => self.query_fulfilled_event(request_id, None, None).await,
            _ => Err(MarketError::RequestNotFulfilled(request_id)),
        }
    }

    /// Returns journal and seal if the request is fulfilled.
    ///
    /// This method will poll the status of the request until it is Fulfilled or Expired.
    /// Polling is done at intervals of `retry_interval` until the request is Fulfilled, Expired or
    /// the optional timeout is reached.
    pub async fn wait_for_request_fulfillment(
        &self,
        request_id: U256,
        retry_interval: Duration,
        expires_at: u64,
    ) -> Result<(Bytes, Bytes), MarketError> {
        loop {
            let status = self.get_status(request_id, Some(expires_at)).await?;
            match status {
                ProofStatus::Expired => return Err(MarketError::RequestHasExpired(request_id)),
                ProofStatus::Fulfilled => {
                    return self.query_fulfilled_event(request_id, None, None).await;
                }
                _ => {
                    tracing::info!(
                        "Request {} status: {:?}. Retrying in {:?}",
                        request_id,
                        status,
                        retry_interval
                    );
                    tokio::time::sleep(retry_interval).await;
                    continue;
                }
            }
        }
    }

    /// Calculates the block number at which the price will be at the given price.
    pub fn block_at_price(&self, offer: &Offer, price: U256) -> Result<u64, MarketError> {
        let max_price = U256::from(offer.maxPrice);
        let min_price = U256::from(offer.minPrice);

        if price > U256::from(max_price) {
            return Err(MarketError::Error(anyhow::anyhow!("Price cannot exceed max price")));
        }

        if price <= min_price {
            return Ok(0);
        }

        let rise = max_price - min_price;
        let run = U256::from(offer.rampUpPeriod);
        let delta = ((price - min_price) * run).div_ceil(rise);
        let delta: u64 = delta.try_into().context("Failed to convert block delta to u64")?;

        Ok(offer.biddingStart + delta)
    }

    /// Calculates the price at the given block number.
    pub fn price_at_block(&self, offer: &Offer, block_numb: u64) -> Result<U256, MarketError> {
        let max_price = U256::from(offer.maxPrice);
        let min_price = U256::from(offer.minPrice);

        if block_numb < offer.biddingStart {
            return Err(MarketError::Error(anyhow!("Block number before bidding start")));
        }

        if block_numb < offer.biddingStart + offer.rampUpPeriod as u64 {
            let rise = max_price - min_price;
            let run = U256::from(offer.rampUpPeriod);
            let delta = U256::from(block_numb) - U256::from(offer.biddingStart);

            Ok(min_price + (delta * rise) / run)
        } else {
            Ok(max_price)
        }
    }

    /// Generates a request index based on the EOA nonce.
    ///
    /// It does not guarantee that the index is not in use by the time the caller uses it.
    pub async fn index_from_nonce(&self) -> Result<u32, MarketError> {
        let nonce = self
            .instance
            .provider()
            .get_transaction_count(self.caller)
            .await
            .context(format!("Failed to get EOA nonce for {:?}", self.caller))?;
        let id: u32 = nonce.try_into().context("Failed to convert nonce to u32")?;
        let request_id = request_id(&self.caller, id);
        match self.get_status(U256::from(request_id), None).await? {
            ProofStatus::Unknown => return Ok(id),
            _ => Err(MarketError::Error(anyhow!("index already in use"))),
        }
    }

    /// Generates a new request ID based on the EOA nonce.
    ///
    /// It does not guarantee that the ID is not in use by the time the caller uses it.
    pub async fn request_id_from_nonce(&self) -> Result<U192, MarketError> {
        let index = self.index_from_nonce().await?;
        Ok(request_id(&self.caller, index))
    }

    /// Returns the image ID and URL of the assessor guest.
    pub async fn image_info(&self) -> Result<(B256, String)> {
        tracing::debug!("Calling imageInfo()");
        let (image_id, image_url) =
            self.instance.imageInfo().call().await.context("call failed")?.into();

        Ok((image_id, image_url))
    }
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use super::ProofMarketService;
    use crate::contracts::{
        test_utils::TestCtx, AssessorJournal, Fulfillment, IProofMarket, Input, InputType, Offer,
        Predicate, PredicateType, ProofStatus, ProvingRequest, Requirements,
    };
    use aggregation_set::{merkle_root, GuestOutput, SetInclusionReceipt, SET_BUILDER_GUEST_ID};
    use alloy::{
        node_bindings::Anvil,
        primitives::{
            aliases::{U192, U96},
            utils::parse_ether,
            Address, Bytes, B256, U256,
        },
        providers::{Provider, ProviderBuilder},
        sol_types::{eip712_domain, Eip712Domain, SolValue},
    };
    use guest_assessor::ASSESSOR_GUEST_ID;
    use guest_util::ECHO_ID;
    use risc0_ethereum_contracts::encode_seal;
    use risc0_zkvm::{
        sha::{Digest, Digestible},
        FakeReceipt, InnerReceipt, Journal, MaybePruned, Receipt, ReceiptClaim,
    };
    use tracing_test::traced_test;
    use url::Url;

    fn test_offer() -> Offer {
        Offer {
            minPrice: U96::from(ether("1")),
            maxPrice: U96::from(ether("2")),
            biddingStart: 100,
            rampUpPeriod: 100,
            timeout: 500,
            lockinStake: U96::from(ether("1")),
        }
    }

    fn ether(value: &str) -> u128 {
        parse_ether(value).unwrap().try_into().unwrap()
    }

    async fn new_request(idx: u32, ctx: &TestCtx) -> ProvingRequest {
        ProvingRequest::new(
            idx,
            &ctx.customer_signer.address(),
            Requirements {
                imageId: to_b256(Digest::from(ECHO_ID)),
                predicate: Predicate {
                    predicateType: PredicateType::PrefixMatch,
                    data: Default::default(),
                },
            },
            "http://image_uri.null",
            Input { inputType: InputType::Inline, data: Bytes::default() },
            Offer {
                minPrice: U96::from(20000000000000u64),
                maxPrice: U96::from(40000000000000u64),
                biddingStart: ctx.customer_provider.get_block_number().await.unwrap(),
                timeout: 100,
                rampUpPeriod: 1,
                lockinStake: U96::from(10),
            },
        )
    }

    fn to_b256(digest: Digest) -> B256 {
        <[u8; 32]>::from(digest).into()
    }

    fn mock_singleton(
        request_id: U256,
        eip712_domain: Eip712Domain,
    ) -> (B256, Bytes, Fulfillment, Bytes) {
        let app_journal = Journal::new(vec![0x41, 0x41, 0x41, 0x41]);
        let app_receipt_claim = ReceiptClaim::ok(ECHO_ID, app_journal.clone().bytes);
        let app_claim_digest = app_receipt_claim.digest();

        let assessor_journal = AssessorJournal {
            requestIds: vec![U192::from(request_id)],
            root: to_b256(app_claim_digest),
            eip712DomainSeparator: eip712_domain.separator(),
        };
        let assesor_receipt_claim =
            ReceiptClaim::ok(ASSESSOR_GUEST_ID, assessor_journal.abi_encode());
        let assessor_claim_digest = assesor_receipt_claim.digest();

        let root = merkle_root(&vec![app_claim_digest, assessor_claim_digest]).unwrap();
        let set_builder_journal = GuestOutput::new(Digest::from(SET_BUILDER_GUEST_ID), root);
        let set_builder_receipt_claim =
            ReceiptClaim::ok(SET_BUILDER_GUEST_ID, set_builder_journal.abi_encode());

        let set_builder_receipt = Receipt::new(
            InnerReceipt::Fake(FakeReceipt::new(set_builder_receipt_claim)),
            set_builder_journal.abi_encode(),
        );
        let set_verifier_seal = encode_seal(&set_builder_receipt).unwrap();

        let set_inclusion_seal = SetInclusionReceipt::from_path(
            ReceiptClaim::ok(ECHO_ID, MaybePruned::Pruned(app_journal.digest())),
            vec![assessor_claim_digest],
        )
        .abi_encode_seal()
        .unwrap();

        let fulfillment = Fulfillment {
            id: U192::from(request_id),
            imageId: to_b256(Digest::from(ECHO_ID)),
            journal: app_journal.bytes.into(),
            seal: set_inclusion_seal.into(),
        };

        let assessor_seal = SetInclusionReceipt::from_path(
            ReceiptClaim::ok(ASSESSOR_GUEST_ID, MaybePruned::Pruned(Digest::ZERO)),
            vec![app_claim_digest],
        )
        .abi_encode_seal()
        .unwrap();

        (to_b256(root), set_verifier_seal.into(), fulfillment, assessor_seal.into())
    }

    #[test]
    fn test_price_at_block() {
        let market = ProofMarketService::new(
            Address::default(),
            ProviderBuilder::default().on_http(Url::from_str("http://rpc.null").unwrap()),
            Address::default(),
        );
        let offer = &test_offer();

        // Cannot calculate price before bidding start
        assert!(market.price_at_block(offer, 99).is_err());

        assert_eq!(market.price_at_block(offer, 100).unwrap(), U256::from(ether("1")));

        assert_eq!(market.price_at_block(offer, 101).unwrap(), U256::from(ether("1.01")));
        assert_eq!(market.price_at_block(offer, 125).unwrap(), U256::from(ether("1.25")));
        assert_eq!(market.price_at_block(offer, 150).unwrap(), U256::from(ether("1.5")));
        assert_eq!(market.price_at_block(offer, 175).unwrap(), U256::from(ether("1.75")));
        assert_eq!(market.price_at_block(offer, 199).unwrap(), U256::from(ether("1.99")));

        assert_eq!(market.price_at_block(offer, 200).unwrap(), U256::from(ether("2")));
        assert_eq!(market.price_at_block(offer, 500).unwrap(), U256::from(ether("2")));
    }

    #[test]
    fn test_block_at_price() {
        let market = ProofMarketService::new(
            Address::default(),
            ProviderBuilder::default().on_http(Url::from_str("http://rpc.null").unwrap()),
            Address::default(),
        );
        let offer = &test_offer();

        assert_eq!(market.block_at_price(offer, U256::from(ether("1"))).unwrap(), 0);

        assert_eq!(market.block_at_price(offer, U256::from(ether("1.01"))).unwrap(), 101);
        assert_eq!(market.block_at_price(offer, U256::from(ether("1.001"))).unwrap(), 101);

        assert_eq!(market.block_at_price(offer, U256::from(ether("1.25"))).unwrap(), 125);
        assert_eq!(market.block_at_price(offer, U256::from(ether("1.5"))).unwrap(), 150);
        assert_eq!(market.block_at_price(offer, U256::from(ether("1.75"))).unwrap(), 175);
        assert_eq!(market.block_at_price(offer, U256::from(ether("1.99"))).unwrap(), 199);
        assert_eq!(market.block_at_price(offer, U256::from(ether("2"))).unwrap(), 200);

        // Price cannot exceed maxPrice
        assert!(market.block_at_price(offer, U256::from(ether("3"))).is_err());
    }

    #[tokio::test]
    async fn test_deposit_withdraw() {
        // Setup anvil
        let anvil = Anvil::new().spawn();

        let ctx = TestCtx::new(&anvil).await.unwrap();

        // Deposit prover balances
        ctx.prover_market.deposit(parse_ether("2").unwrap()).await.unwrap();
        assert_eq!(
            ctx.prover_market.balance_of(ctx.prover_signer.address()).await.unwrap(),
            parse_ether("2").unwrap()
        );

        // Withdraw prover balances
        ctx.prover_market.withdraw(parse_ether("2").unwrap()).await.unwrap();
        assert_eq!(
            ctx.prover_market.balance_of(ctx.prover_signer.address()).await.unwrap(),
            U256::ZERO
        );

        // Withdraw when balance is zero
        assert!(ctx.prover_market.withdraw(parse_ether("2").unwrap()).await.is_err());
    }

    #[tokio::test]
    async fn test_submit_request() {
        // Setup anvil
        let anvil = Anvil::new().spawn();

        let ctx = TestCtx::new(&anvil).await.unwrap();

        let request = new_request(1, &ctx).await;

        let request_id =
            ctx.customer_market.submit_request(&request, &ctx.customer_signer).await.unwrap();

        // fetch logs and check if the event was emitted
        let logs = ctx.customer_market.instance().RequestSubmitted_filter().query().await.unwrap();

        let (_, log) = logs.first().unwrap();
        let log = log.log_decode::<IProofMarket::RequestSubmitted>().unwrap();
        assert!(log.inner.data.request.id == U192::from(request_id));
    }

    #[tokio::test]
    #[traced_test]
    async fn test_e2e() {
        // Setup anvil
        let anvil = Anvil::new().spawn();

        let ctx = TestCtx::new(&anvil).await.unwrap();

        let eip712_domain = eip712_domain! {
            name: "IProofMarket",
            version: "1",
            chain_id: anvil.chain_id(),
            verifying_contract: *ctx.customer_market.instance().address(),
        };

        let request = new_request(1, &ctx).await;
        let expires_at = request.expires_at();

        let request_id =
            ctx.customer_market.submit_request(&request, &ctx.customer_signer).await.unwrap();

        // fetch logs to retrieve the customer signature from the event
        let logs = ctx.customer_market.instance().RequestSubmitted_filter().query().await.unwrap();

        let (_, log) = logs.first().unwrap();
        let log = log.log_decode::<IProofMarket::RequestSubmitted>().unwrap();
        let request = log.inner.data.request;
        let customer_sig = log.inner.data.clientSignature;

        // Deposit prover balances
        ctx.prover_market.deposit(parse_ether("1").unwrap()).await.unwrap();

        // Lockin the request
        ctx.prover_market.lockin_request(&request, &customer_sig, None).await.unwrap();
        assert!(ctx.customer_market.is_locked_in(request_id).await.unwrap());
        assert!(
            ctx.customer_market.get_status(request_id, Some(expires_at)).await.unwrap()
                == ProofStatus::Locked
        );

        // mock the fulfillment
        let (root, set_verifier_seal, fulfillment, market_seal) =
            mock_singleton(request_id, eip712_domain);

        // publish the committed root
        ctx.set_verifier.submit_merkle_root(root, set_verifier_seal).await.unwrap();

        // fulfill the request
        ctx.prover_market.fulfill(&fulfillment, &market_seal).await.unwrap();
        assert!(ctx.customer_market.is_fulfilled(request_id).await.unwrap());

        // retrieve journal and seal from the fulfilled request
        let (journal, seal) =
            ctx.customer_market.get_request_fulfillment(request_id).await.unwrap();

        assert_eq!(journal, fulfillment.journal);
        assert_eq!(seal, fulfillment.seal);
    }

    #[tokio::test]
    #[traced_test]
    async fn test_e2e_merged_submit_fulfill() {
        // Setup anvil
        let anvil = Anvil::new().spawn();

        let ctx = TestCtx::new(&anvil).await.unwrap();

        let eip712_domain = eip712_domain! {
            name: "IProofMarket",
            version: "1",
            chain_id: anvil.chain_id(),
            verifying_contract: *ctx.customer_market.instance().address(),
        };

        let request = new_request(1, &ctx).await;
        let expires_at = request.expires_at();

        let request_id =
            ctx.customer_market.submit_request(&request, &ctx.customer_signer).await.unwrap();

        // fetch logs to retrieve the customer signature from the event
        let logs = ctx.customer_market.instance().RequestSubmitted_filter().query().await.unwrap();

        let (_, log) = logs.first().unwrap();
        let log = log.log_decode::<IProofMarket::RequestSubmitted>().unwrap();
        let request = log.inner.data.request;
        let customer_sig = log.inner.data.clientSignature;

        // Deposit prover balances
        ctx.prover_market.deposit(parse_ether("1").unwrap()).await.unwrap();

        // Lockin the request
        ctx.prover_market.lockin_request(&request, &customer_sig, None).await.unwrap();
        assert!(ctx.customer_market.is_locked_in(request_id).await.unwrap());
        assert!(
            ctx.customer_market.get_status(request_id, Some(expires_at)).await.unwrap()
                == ProofStatus::Locked
        );

        // mock the fulfillment
        let (root, set_verifier_seal, fulfillment, market_seal) =
            mock_singleton(request_id, eip712_domain);

        let fulfillments = vec![fulfillment];
        // publish the committed root + fulfillments
        ctx.prover_market
            .submit_merkle_and_fulfill(root, set_verifier_seal, fulfillments.clone(), market_seal)
            .await
            .unwrap();

        // retrieve journal and seal from the fulfilled request
        let (journal, seal) =
            ctx.customer_market.get_request_fulfillment(request_id).await.unwrap();

        assert_eq!(journal, fulfillments[0].journal);
        assert_eq!(seal, fulfillments[0].seal);
    }
}
