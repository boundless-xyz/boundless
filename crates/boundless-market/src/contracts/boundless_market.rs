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

use std::{
    fmt::Debug,
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};

use alloy::{
    consensus::BlockHeader,
    eips::BlockNumberOrTag,
    network::Ethereum,
    primitives::{Address, Bytes, B256, U256},
    providers::Provider,
    rpc::types::{BlockTransactionsKind, Log, TransactionReceipt},
    signers::Signer,
    transports::Transport,
};
use alloy_sol_types::SolEvent;
use anyhow::{anyhow, Context, Result};
use thiserror::Error;

use crate::contracts::token::{IERC20Permit, IHitPoints::IHitPointsErrors, Permit, IERC20};

use super::{
    eip712_domain, request_id, EIP721DomainSaltless, Fulfillment,
    IBoundlessMarket::{self, IBoundlessMarketInstance},
    IBoundlessMarketErrors, Offer, ProofRequest, ProofStatus, RequestError, TxnErr,
    TXN_CONFIRM_TIMEOUT,
};

/// Boundless market errors.
#[derive(Error, Debug)]
pub enum MarketError {
    /// Transaction error.
    #[error("Transaction error: {0}")]
    TxnError(#[from] TxnErr),

    /// Request not fulfilled.
    #[error("Request is not fulfilled 0x{0:x}")]
    RequestNotFulfilled(U256),

    /// Request has expired.
    #[error("Request has expired 0x{0:x}")]
    RequestHasExpired(U256),

    /// Request malformed.
    #[error("Request error {0}")]
    RequestError(#[from] RequestError),

    /// Request address does not match with signer.
    #[error("Request address does not match with signer {0} - {0}")]
    AddressMismatch(Address, Address),

    /// Proof not found.
    #[error("Proof not found for request in events logs 0x{0:x}")]
    ProofNotFound(U256),

    /// Request not found.
    #[error("Request not found in event logs 0x{0:x}")]
    RequestNotFound(U256),

    /// Lockin reverted, possibly outbid.
    #[error("Lockin reverted, possibly outbid: txn_hash: {0}")]
    LockRevert(B256),

    /// General market error.
    #[error("Market error: {0}")]
    Error(#[from] anyhow::Error),

    /// Timeout reached.
    #[error("Timeout: 0x{0:x}")]
    TimeoutReached(U256),
}

impl From<alloy::contract::Error> for MarketError {
    fn from(err: alloy::contract::Error) -> Self {
        MarketError::Error(IBoundlessMarketErrors::decode_error(err).into())
    }
}

/// Proof market service.
pub struct BoundlessMarketService<T, P> {
    instance: IBoundlessMarketInstance<T, P, Ethereum>,
    // Chain ID with caching to ensure we fetch it at most once.
    chain_id: AtomicU64,
    caller: Address,
    timeout: Duration,
    event_query_config: EventQueryConfig,
}

impl<T, P> Clone for BoundlessMarketService<T, P>
where
    IBoundlessMarketInstance<T, P, Ethereum>: Clone,
{
    fn clone(&self) -> Self {
        Self {
            instance: self.instance.clone(),
            chain_id: self.chain_id.load(Ordering::Relaxed).into(),
            caller: self.caller,
            timeout: self.timeout,
            event_query_config: self.event_query_config.clone(),
        }
    }
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
        // Default values chosen based on the docs and pricing of requests on common RPC providers.
        Self { max_iterations: 100, block_range: 1000 }
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

fn extract_tx_log<E: SolEvent + Debug + Clone>(
    receipt: &TransactionReceipt,
) -> Result<Log<E>, anyhow::Error> {
    let logs = receipt
        .inner
        .logs()
        .iter()
        .filter_map(|log| {
            if log.topic0().map(|topic| E::SIGNATURE_HASH == *topic).unwrap_or(false) {
                Some(
                    log.log_decode::<E>()
                        .with_context(|| format!("failed to decode event {}", E::SIGNATURE)),
                )
            } else {
                tracing::debug!(
                    "skipping log on receipt; does not match {}: {log:?}",
                    E::SIGNATURE
                );
                None
            }
        })
        .collect::<Result<Vec<_>>>()?;

    match &logs[..] {
        [log] => Ok(log.clone()),
        [] => Err(anyhow!("transaction did not emit event {}", E::SIGNATURE)),
        _ => Err(anyhow!(
            "transaction emitted more than one event with signature {}, {:#?}",
            E::SIGNATURE,
            logs
        )),
    }
}

impl<T, P> BoundlessMarketService<T, P>
where
    T: Transport + Clone,
    P: Provider<T, Ethereum> + 'static + Clone,
{
    /// Creates a new Boundless market service.
    pub fn new(address: Address, provider: P, caller: Address) -> Self {
        let instance = IBoundlessMarket::new(address, provider);

        Self {
            instance,
            chain_id: AtomicU64::new(0),
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

    /// Returns the market contract instance.
    pub fn instance(&self) -> &IBoundlessMarketInstance<T, P, Ethereum> {
        &self.instance
    }

    /// Returns the caller address.
    pub fn caller(&self) -> Address {
        self.caller
    }

    /// Get the EIP-712 domain associated with the market contract.
    ///
    /// If not cached, this function will fetch the chain ID with an RPC call.
    pub async fn eip712_domain(&self) -> Result<EIP721DomainSaltless, MarketError> {
        Ok(eip712_domain(*self.instance.address(), self.get_chain_id().await?))
    }

    /// Deposit Ether into the market to pay for proof and/or lockin stake.
    pub async fn deposit(&self, value: U256) -> Result<(), MarketError> {
        tracing::debug!("Calling deposit() value: {value}");
        let call = self.instance.deposit().value(value);
        let pending_tx = call.send().await?;
        tracing::debug!("Broadcasting deposit tx {}", pending_tx.tx_hash());
        let tx_hash = pending_tx
            .with_timeout(Some(self.timeout))
            .watch()
            .await
            .context("failed to confirm tx")?;
        tracing::debug!("Submitted deposit {}", tx_hash);

        Ok(())
    }

    /// Withdraw Ether from the market.
    pub async fn withdraw(&self, amount: U256) -> Result<(), MarketError> {
        tracing::debug!("Calling withdraw({amount})");
        let call = self.instance.withdraw(amount);
        let pending_tx = call.send().await?;
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
        let balance = self.instance.balanceOf(account).call().await?._0;

        Ok(balance)
    }

    /// Returns the frozen state of the given account.
    pub async fn account_is_frozen(&self, account: Address) -> Result<bool, MarketError> {
        tracing::debug!("Calling accountIdFrozen({account})");
        let frozen = self.instance.accountIsFrozen(account).call().await?._0;

        Ok(frozen)
    }

    /// Unfreeze the account.
    /// This function can only be called by the account owner.
    pub async fn unfreeze_account(&self) -> Result<(), MarketError> {
        tracing::debug!("Calling unfreezeAccount()");
        let call = self.instance.unfreezeAccount().from(self.caller);
        let pending_tx = call.send().await?;
        tracing::debug!("Broadcasting unfreezeAccount tx {}", pending_tx.tx_hash());
        let tx_hash = pending_tx
            .with_timeout(Some(self.timeout))
            .watch()
            .await
            .context("failed to confirm tx")?;
        tracing::debug!("Submitted unfreezeAccount {}", tx_hash);

        Ok(())
    }

    /// Submit a request such that it is publicly available for provers to evaluate and bid
    /// on. Includes the specified value, which will be deposited to the account of msg.sender.
    pub async fn submit_request_with_value(
        &self,
        request: &ProofRequest,
        signer: &impl Signer,
        value: impl Into<U256>,
    ) -> Result<U256, MarketError> {
        tracing::debug!("calling submitRequest({:x?})", request);
        let client_address = request.client_address()?;
        if client_address != signer.address() {
            return Err(MarketError::AddressMismatch(client_address, signer.address()));
        };
        let chain_id = self.get_chain_id().await.context("failed to get chain ID")?;
        let client_sig = request
            .sign_request(signer, *self.instance.address(), chain_id)
            .await
            .context("failed to sign request")?;
        let call = self
            .instance
            .submitRequest(request.clone(), client_sig.as_bytes().into())
            .from(self.caller)
            .value(value.into());
        let pending_tx = call.send().await?;
        tracing::debug!("broadcasting tx {}", pending_tx.tx_hash());

        let receipt = pending_tx
            .with_timeout(Some(self.timeout))
            .get_receipt()
            .await
            .context("failed to confirm tx")?;

        // Look for the logs for submitting the transaction.
        let log = extract_tx_log::<IBoundlessMarket::RequestSubmitted>(&receipt)?;
        Ok(U256::from(log.inner.data.request.id))
    }

    /// Submit a request such that it is publicly available for provers to evaluate and bid
    /// on. Deposits funds to the client account if there are not enough to cover the max price on
    /// the offer.
    pub async fn submit_request(
        &self,
        request: &ProofRequest,
        signer: &impl Signer,
    ) -> Result<U256, MarketError> {
        let balance = self
            .balance_of(signer.address())
            .await
            .context("failed to get whether the client balance can cover the offer max price")?;
        let max_price = U256::from(request.offer.maxPrice);
        let value = if balance > max_price { U256::ZERO } else { U256::from(max_price) - balance };
        self.submit_request_with_value(request, signer, value).await
    }

    /// Lock the request to the prover, giving them exclusive rights to be paid to
    /// fulfill this request, and also making them subject to slashing penalties if they fail to
    /// deliver. At this point, the price for fulfillment is also set, based on the reverse Dutch
    /// auction parameters and the block at which this transaction is processed.
    ///
    /// This method should be called from the address of the prover.
    pub async fn lockin_request(
        &self,
        request: &ProofRequest,
        client_sig: &Bytes,
        priority_gas: Option<u64>,
    ) -> Result<u64, MarketError> {
        tracing::debug!("Calling requestIsLocked({:x})", request.id);
        let is_locked_in: bool =
            self.instance.requestIsLocked(request.id).call().await.context("call failed")?._0;
        if is_locked_in {
            return Err(MarketError::Error(anyhow!("request is already locked-in")));
        }

        tracing::debug!("Calling lockin({:x?}, {:x?})", request, client_sig);

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

        let pending_tx = call.send().await?;

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

    /// Lock the request to the prover, giving them exclusive rights to be paid to
    /// fulfill this request, and also making them subject to slashing penalties if they fail to
    /// deliver. At this point, the price for fulfillment is also set, based on the reverse Dutch
    /// auction parameters and the block at which this transaction is processed.
    ///
    /// This method uses the provided signature to authenticate the prover.
    pub async fn lockin_request_with_sig(
        &self,
        request: &ProofRequest,
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

        tracing::debug!(
            "Calling lockinWithSig({:x?}, {:x?}, {:x?})",
            request,
            client_sig,
            prover_sig
        );

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
    pub async fn slash(
        &self,
        request_id: U256,
    ) -> Result<IBoundlessMarket::ProverSlashed, MarketError> {
        tracing::debug!("Calling slash({:x?})", request_id);
        let call = self.instance.slash(request_id).from(self.caller);
        let pending_tx = call.send().await?;
        tracing::debug!("Broadcasting tx {}", pending_tx.tx_hash());

        let receipt = pending_tx
            .with_timeout(Some(self.timeout))
            .get_receipt()
            .await
            .context("failed to confirm tx")?;

        let log = extract_tx_log::<IBoundlessMarket::ProverSlashed>(&receipt)?;
        Ok(log.inner.data)
    }

    /// Fulfill a request by delivering the proof for the application.
    ///
    /// Upon proof verification, the prover is paid as long as the requirements are met, including:
    ///
    /// * Seal for the assessor proof is valid, verifying that the order's requirements are met.
    /// * The order has not expired.
    /// * The order is not locked by a different prover.
    /// * A prover has not been paid for the job already.
    /// * If not locked, the client has sufficient funds.
    ///
    /// When fulfillment has `require_payment` set to true, the transaction will revert if the
    /// payment is not sent. Otherwise, an event will be logged on the transaction and returned.
    pub async fn fulfill(
        &self,
        fulfillment: &Fulfillment,
        assessor_seal: &Bytes,
        prover_address: Address,
    ) -> Result<Option<Log<IBoundlessMarket::PaymentRequirementsFailed>>, MarketError> {
        tracing::debug!("Calling fulfill({:x?},{:x?})", fulfillment, assessor_seal);
        let call = self
            .instance
            .fulfill(fulfillment.clone(), assessor_seal.clone(), prover_address)
            .from(self.caller);
        let pending_tx = call.send().await?;
        tracing::debug!("Broadcasting tx {}", pending_tx.tx_hash());

        let receipt = pending_tx
            .with_timeout(Some(self.timeout))
            .get_receipt()
            .await
            .context("failed to confirm tx")?;

        tracing::info!(
            "Submitted proof for request {:x}: {:x}",
            fulfillment.id,
            receipt.transaction_hash
        );

        // Look for PaymentRequirementsFailed logs.
        let mut logs = receipt.inner.logs().iter().filter_map(|log| {
            let log = log.log_decode::<IBoundlessMarket::PaymentRequirementsFailed>();
            log.ok()
        });
        let maybe_log = logs.nth(0);
        if logs.next().is_some() {
            return Err(anyhow!(
                "more than one PaymentRequirementsFailed event on single fullfillment tx"
            )
            .into());
        }
        if fulfillment.requirePayment && maybe_log.is_some() {
            return Err(anyhow!(
                "bug in market contract; payment failed and require_payment is true"
            )
            .into());
        }

        Ok(maybe_log)
    }

    /// Fulfill a batch of requests by delivering the proof for each application.
    ///
    /// See [BoundlessMarketService::fulfill] for more details.
    pub async fn fulfill_batch(
        &self,
        fulfillments: Vec<Fulfillment>,
        assessor_seal: Bytes,
        prover_address: Address,
    ) -> Result<Vec<Log<IBoundlessMarket::PaymentRequirementsFailed>>, MarketError> {
        let fill_ids = fulfillments.iter().map(|fill| fill.id).collect::<Vec<_>>();
        tracing::debug!("Calling fulfillBatch({fulfillments:x?}, {assessor_seal:x})");
        let call = self
            .instance
            .fulfillBatch(fulfillments, assessor_seal, prover_address)
            .from(self.caller);
        tracing::debug!("Calldata: {:x}", call.calldata());
        let pending_tx = call.send().await?;
        tracing::debug!("Broadcasting tx {}", pending_tx.tx_hash());

        let receipt = pending_tx
            .with_timeout(Some(self.timeout))
            .get_receipt()
            .await
            .context("failed to confirm tx")?;

        // Look for PaymentRequirementsFailed logs.
        let logs = receipt
            .inner
            .logs()
            .iter()
            .filter_map(|log| {
                let log = log.log_decode::<IBoundlessMarket::PaymentRequirementsFailed>();
                log.ok()
            })
            .collect();

        tracing::info!("Submitted proof for batch {:?}: {}", fill_ids, receipt.transaction_hash);

        Ok(logs)
    }

    /// Combined function to submit a new merkle root to the set-verifier and call `fulfillBatch`.
    /// Useful to reduce the transaction count for fulfillments
    pub async fn submit_merkle_and_fulfill(
        &self,
        root: B256,
        seal: Bytes,
        fulfillments: Vec<Fulfillment>,
        assessor_seal: Bytes,
        prover_address: Address,
    ) -> Result<(), MarketError> {
        tracing::debug!("Calling submitRootAndFulfillBatch({root:?}, {seal:x}, {fulfillments:?}, {assessor_seal:x})");
        let call = self
            .instance
            .submitRootAndFulfillBatch(root, seal, fulfillments, assessor_seal, prover_address)
            .from(self.caller);
        tracing::debug!("Calldata: {}", call.calldata());
        let pending_tx = call.send().await?;
        tracing::debug!("Broadcasting tx {}", pending_tx.tx_hash());
        let tx_receipt = pending_tx
            .with_timeout(Some(self.timeout))
            .get_receipt()
            .await
            .context("failed to confirm tx")?;

        tracing::info!("Submitted merkle root and proof for batch {}", tx_receipt.transaction_hash);

        Ok(())
    }

    /// A combined call to `IBoundlessMarket.priceRequest` and `IBoundlessMarket.fulfillBatch`.
    /// The caller should provide the signed request and signature for each unlocked request they
    /// want to fulfill. Payment for unlocked requests will go to the provided `prover` address.
    pub async fn price_and_fulfill_batch(
        &self,
        requests: Vec<ProofRequest>,
        client_sigs: Vec<Bytes>,
        fulfillments: Vec<Fulfillment>,
        assessor_seal: Bytes,
        prover_address: Address,
        priority_gas: Option<u64>,
    ) -> Result<(), MarketError> {
        for request in requests.iter() {
            tracing::debug!("Calling requestIsLocked({:x})", request.id);
            let is_locked_in: bool =
                self.instance.requestIsLocked(request.id).call().await.context("call failed")?._0;
            if is_locked_in {
                return Err(MarketError::Error(anyhow!(
                    "request {:x} is already locked-in",
                    request.id
                )));
            }
        }

        tracing::debug!("Calling priceAndFulfillBatch({fulfillments:?}, {assessor_seal:x})");

        let mut call = self
            .instance
            .priceAndFulfillBatch(
                requests,
                client_sigs,
                fulfillments,
                assessor_seal,
                prover_address,
            )
            .from(self.caller);
        tracing::debug!("Calldata: {}", call.calldata());

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

        let pending_tx = call.send().await?;
        tracing::debug!("Broadcasting tx {}", pending_tx.tx_hash());

        let tx_receipt = pending_tx
            .with_timeout(Some(self.timeout))
            .get_receipt()
            .await
            .context("failed to confirm tx")?;

        tracing::info!("Fulfilled proof for batch {}", tx_receipt.transaction_hash);

        Ok(())
    }

    /// Checks if a request is locked in.
    pub async fn is_locked_in(&self, request_id: U256) -> Result<bool, MarketError> {
        tracing::debug!("Calling requestIsLocked({:x})", request_id);
        let res = self.instance.requestIsLocked(request_id).call().await?;

        Ok(res._0)
    }

    /// Checks if a request is fulfilled.
    pub async fn is_fulfilled(&self, request_id: U256) -> Result<bool, MarketError> {
        tracing::debug!("Calling requestIsFulfilled({:x})", request_id);
        let res = self.instance.requestIsFulfilled(request_id).call().await?;

        Ok(res._0)
    }

    /// Checks if a request is slashed.
    pub async fn is_slashed(&self, request_id: U256) -> Result<bool, MarketError> {
        tracing::debug!("Calling requestIsSlashed({:x})", request_id);
        let res = self.instance.requestIsSlashed(request_id).call().await?;

        Ok(res._0)
    }

    /// Returns the [ProofStatus] of a request.
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
            let deadline = self.instance.requestDeadline(request_id).call().await?._0;
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

    /// Query the ProofDelivered event based on request ID and block options.
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
            let mut event_filter = self.instance.ProofDelivered_filter();
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

    /// Query the RequestSubmitted event based on request ID and block options.
    ///
    /// For each iteration, we query a range of blocks.
    /// If the event is not found, we move the range down and repeat until we find the event.
    /// If the event is not found after the configured max iterations, we return an error.
    /// The default range is set to 100 blocks for each iteration, and the default maximum number of
    /// iterations is 100. This means that the search will cover a maximum of 10,000 blocks.
    /// Optionally, you can specify a lower and upper bound to limit the search range.
    async fn query_request_submitted_event(
        &self,
        request_id: U256,
        lower_bound: Option<u64>,
        upper_bound: Option<u64>,
    ) -> Result<(ProofRequest, Bytes), MarketError> {
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
            let mut event_filter = self.instance.RequestSubmitted_filter();
            event_filter.filter = event_filter
                .filter
                .topic1(request_id)
                .from_block(lower_block)
                .to_block(upper_block);

            // Query the logs for the event
            let logs = event_filter.query().await?;

            if let Some((log, _)) = logs.first() {
                return Ok((log.request.clone(), log.clientSignature.clone()));
            }

            // Move the upper_block down for the next iteration
            upper_block = lower_block.saturating_sub(1);
        }

        // Return error if no logs are found after all iterations
        Err(MarketError::RequestNotFound(request_id))
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
    pub async fn get_submitted_request(
        &self,
        request_id: U256,
        tx_hash: Option<B256>,
    ) -> Result<(ProofRequest, Bytes), MarketError> {
        if let Some(tx_hash) = tx_hash {
            let receipt = self
                .instance
                .provider()
                .get_transaction_receipt(tx_hash)
                .await
                .context("Failed to get transaction receipt")?
                .context("Transaction not found")?;
            let logs = receipt.inner.logs().iter().filter_map(|log| {
                let log = log.log_decode::<IBoundlessMarket::RequestSubmitted>();
                log.ok()
            });
            for log in logs {
                if U256::from(log.inner.data.request.id) == request_id {
                    return Ok((log.inner.data.request, log.inner.data.clientSignature));
                }
            }
        }
        self.query_request_submitted_event(request_id, None, None).await
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
                        "Request {:x} status: {:?}. Retrying in {:?}",
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
        match self.get_status(request_id, None).await? {
            ProofStatus::Unknown => Ok(id),
            _ => Err(MarketError::Error(anyhow!("index already in use"))),
        }
    }

    /// Generates a new request ID based on the EOA nonce.
    ///
    /// It does not guarantee that the ID is not in use by the time the caller uses it.
    pub async fn request_id_from_nonce(&self) -> Result<U256, MarketError> {
        let index = self.index_from_nonce().await?;
        Ok(request_id(&self.caller, index))
    }

    /// Randomly generates a request index.
    ///
    /// It retries up to 10 times to generate a unique index, after which it returns an error.
    /// It does not guarantee that the index is not in use by the time the caller uses it.
    pub async fn index_from_rand(&self) -> Result<u32, MarketError> {
        let attempts = 10usize;
        for _ in 0..attempts {
            let id: u32 = rand::random();
            let request_id = request_id(&self.caller, id);
            match self.get_status(request_id, None).await? {
                ProofStatus::Unknown => return Ok(id),
                _ => continue,
            }
        }
        Err(MarketError::Error(anyhow!(
            "failed to generate a unique index after {attempts} attempts"
        )))
    }

    /// Randomly generates a new request ID.
    ///
    /// It does not guarantee that the ID is not in use by the time the caller uses it.
    pub async fn request_id_from_rand(&self) -> Result<U256, MarketError> {
        let index = self.index_from_rand().await?;
        Ok(request_id(&self.caller, index))
    }

    /// Returns the image ID and URL of the assessor guest.
    pub async fn image_info(&self) -> Result<(B256, String)> {
        tracing::debug!("Calling imageInfo()");
        let (image_id, image_url) =
            self.instance.imageInfo().call().await.context("call failed")?.into();

        Ok((image_id, image_url))
    }

    /// Get the chain ID.
    ///
    /// This function implements caching to save the chain ID after the first successful fetch.
    pub async fn get_chain_id(&self) -> Result<u64, MarketError> {
        let mut id = self.chain_id.load(Ordering::Relaxed);
        if id != 0 {
            return Ok(id);
        }
        id = self.instance.provider().get_chain_id().await.context("failed to get chain ID")?;
        self.chain_id.store(id, Ordering::Relaxed);
        Ok(id)
    }

    /// Approve a spender to spend `value` amount of HitPoints on behalf of the caller.
    pub async fn approve_deposit_stake(&self, value: U256) -> Result<()> {
        let spender = *self.instance.address();
        tracing::debug!("Calling approve({:?}, {})", spender, value);
        let token_address = self
            .instance
            .STAKE_TOKEN_CONTRACT()
            .call()
            .await
            .context("STAKE_TOKEN_CONTRACT call failed")?
            ._0;
        let contract = IERC20::new(token_address, self.instance.provider());
        let call = contract.approve(spender, value).from(self.caller);
        let pending_tx = call.send().await.map_err(IHitPointsErrors::decode_error)?;
        tracing::debug!("Broadcasting tx {}", pending_tx.tx_hash());
        let tx_hash = pending_tx
            .with_timeout(Some(self.timeout))
            .watch()
            .await
            .context("failed to confirm tx")?;

        tracing::info!("Approved {} to spend {}: {}", spender, value, tx_hash);

        Ok(())
    }

    /// Deposit stake into the market to pay for lockin stake.
    ///
    /// Before calling this method, the account owner must first approve
    /// the Boundless market contract as an allowed spender by calling `approve_deposit_stake`.    
    pub async fn deposit_stake(&self, value: U256) -> Result<(), MarketError> {
        tracing::debug!("Calling depositStake({})", value);
        let call = self.instance.depositStake(value);
        let pending_tx = call.send().await?;
        tracing::debug!("Broadcasting stake deposit tx {}", pending_tx.tx_hash());
        let tx_hash = pending_tx
            .with_timeout(Some(self.timeout))
            .watch()
            .await
            .context("failed to confirm tx")?;
        tracing::debug!("Submitted stake deposit {}", tx_hash);
        Ok(())
    }

    /// Permit and deposit stake into the market to pay for lockin stake.
    ///
    /// This method will send a single transaction.
    pub async fn deposit_stake_with_permit(
        &self,
        value: U256,
        signer: &impl Signer,
    ) -> Result<(), MarketError> {
        let token_address = self
            .instance
            .STAKE_TOKEN_CONTRACT()
            .call()
            .await
            .context("STAKE_TOKEN_CONTRACT call failed")?
            ._0;
        let contract = IERC20Permit::new(token_address, self.instance.provider());
        let call = contract.nonces(self.caller());
        let nonce = call.call().await.map_err(IHitPointsErrors::decode_error)?._0;
        let block = self
            .instance
            .provider()
            .get_block_by_number(BlockNumberOrTag::Latest, BlockTransactionsKind::Hashes)
            .await
            .context("failed to get block")?
            .context("failed to get block")?;
        let deadline = U256::from(block.header.timestamp() + 1000);
        let permit = Permit {
            owner: self.caller(),
            spender: *self.instance().address(),
            value,
            nonce,
            deadline,
        };
        tracing::debug!("Permit: {:?}", permit);
        let chain_id = self.get_chain_id().await?;
        let sig = permit.sign(signer, token_address, chain_id).await?.as_bytes();
        let r = B256::from_slice(&sig[..32]);
        let s = B256::from_slice(&sig[32..64]);
        let v: u8 = sig[64];
        tracing::debug!("Calling depositStakeWithPermit({})", value);
        let call = self.instance.depositStakeWithPermit(value, deadline, v, r, s);
        let pending_tx = call.send().await?;
        tracing::debug!("Broadcasting stake deposit tx {}", pending_tx.tx_hash());
        let tx_hash = pending_tx
            .with_timeout(Some(self.timeout))
            .watch()
            .await
            .context("failed to confirm tx")?;
        tracing::debug!("Submitted stake deposit {}", tx_hash);
        Ok(())
    }

    /// Withdraw stake from the market.
    pub async fn withdraw_stake(&self, value: U256) -> Result<(), MarketError> {
        tracing::debug!("Calling withdrawStake({})", value);
        let call = self.instance.withdrawStake(value);
        let pending_tx = call.send().await?;
        tracing::debug!("Broadcasting stake withdraw tx {}", pending_tx.tx_hash());
        let tx_hash = pending_tx
            .with_timeout(Some(self.timeout))
            .watch()
            .await
            .context("failed to confirm tx")?;
        tracing::debug!("Submitted stake withdraw {}", tx_hash);
        Ok(())
    }

    /// Returns the deposited balance, in HP, of the given account.
    pub async fn balance_of_stake(&self, account: Address) -> Result<U256, MarketError> {
        tracing::debug!("Calling balanceOfStake({})", account);
        let balance = self.instance.balanceOfStake(account).call().await.context("call failed")?._0;
        Ok(balance)
    }
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use super::BoundlessMarketService;
    use crate::contracts::{
        hit_points::default_allowance, test_utils::TestCtx, AssessorJournal, Fulfillment,
        IBoundlessMarket, Input, InputType, Offer, Predicate, PredicateType, ProofRequest,
        ProofStatus, Requirements,
    };
    use alloy::{
        node_bindings::Anvil,
        primitives::{aliases::U160, utils::parse_ether, Address, Bytes, B256, U256},
        providers::{Provider, ProviderBuilder},
        sol_types::{eip712_domain, Eip712Domain, SolStruct, SolValue},
    };
    use guest_assessor::ASSESSOR_GUEST_ID;
    use guest_set_builder::SET_BUILDER_ID;
    use guest_util::ECHO_ID;
    use risc0_aggregation::{
        merkle_root, GuestOutput, SetInclusionReceipt, SetInclusionReceiptVerifierParameters,
    };
    use risc0_ethereum_contracts::encode_seal;
    use risc0_zkvm::{
        sha::{Digest, Digestible},
        FakeReceipt, InnerReceipt, Journal, MaybePruned, Receipt, ReceiptClaim,
    };
    use url::Url;

    fn ether(value: &str) -> U256 {
        parse_ether(value).unwrap().try_into().unwrap()
    }

    fn test_offer() -> Offer {
        Offer {
            minPrice: ether("1"),
            maxPrice: ether("2"),
            biddingStart: 100,
            rampUpPeriod: 100,
            timeout: 500,
            lockinStake: ether("1"),
        }
    }

    async fn new_request(idx: u32, ctx: &TestCtx) -> ProofRequest {
        ProofRequest::new(
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
                minPrice: U256::from(20000000000000u64),
                maxPrice: U256::from(40000000000000u64),
                biddingStart: ctx.customer_provider.get_block_number().await.unwrap(),
                timeout: 100,
                rampUpPeriod: 1,
                lockinStake: U256::from(10),
            },
        )
    }

    fn to_b256(digest: Digest) -> B256 {
        <[u8; 32]>::from(digest).into()
    }

    fn mock_singleton(
        request: &ProofRequest,
        eip712_domain: Eip712Domain,
        prover: Address,
    ) -> (B256, Bytes, Fulfillment, Bytes) {
        let app_journal = Journal::new(vec![0x41, 0x41, 0x41, 0x41]);
        let app_receipt_claim = ReceiptClaim::ok(ECHO_ID, app_journal.clone().bytes);
        let app_claim_digest = app_receipt_claim.digest();

        let assessor_journal = AssessorJournal {
            requestDigests: vec![request.eip712_signing_hash(&eip712_domain)],
            root: to_b256(app_claim_digest),
            prover,
        };
        let assesor_receipt_claim =
            ReceiptClaim::ok(ASSESSOR_GUEST_ID, assessor_journal.abi_encode());
        let assessor_claim_digest = assesor_receipt_claim.digest();

        let root = merkle_root(&vec![app_claim_digest, assessor_claim_digest]);
        let set_builder_journal = GuestOutput::new(Digest::from(SET_BUILDER_ID), root);
        let set_builder_receipt_claim =
            ReceiptClaim::ok(SET_BUILDER_ID, set_builder_journal.abi_encode());

        let set_builder_receipt = Receipt::new(
            InnerReceipt::Fake(FakeReceipt::new(set_builder_receipt_claim)),
            set_builder_journal.abi_encode(),
        );
        let set_verifier_seal = encode_seal(&set_builder_receipt).unwrap();

        let verifier_parameters =
            SetInclusionReceiptVerifierParameters { image_id: Digest::from(SET_BUILDER_ID) };
        let set_inclusion_seal = SetInclusionReceipt::from_path_with_verifier_params(
            ReceiptClaim::ok(ECHO_ID, MaybePruned::Pruned(app_journal.digest())),
            vec![assessor_claim_digest],
            verifier_parameters.digest(),
        )
        .abi_encode_seal()
        .unwrap();

        let fulfillment = Fulfillment {
            id: request.id,
            requestDigest: request.eip712_signing_hash(&eip712_domain),
            imageId: to_b256(Digest::from(ECHO_ID)),
            journal: app_journal.bytes.into(),
            seal: set_inclusion_seal.into(),
            requirePayment: true,
        };

        let assessor_seal = SetInclusionReceipt::from_path_with_verifier_params(
            ReceiptClaim::ok(ASSESSOR_GUEST_ID, MaybePruned::Pruned(Digest::ZERO)),
            vec![app_claim_digest],
            verifier_parameters.digest(),
        )
        .abi_encode_seal()
        .unwrap();

        (to_b256(root), set_verifier_seal.into(), fulfillment, assessor_seal.into())
    }

    #[test]
    fn test_price_at_block() {
        let market = BoundlessMarketService::new(
            Address::default(),
            ProviderBuilder::default().on_http(Url::from_str("http://rpc.null").unwrap()),
            Address::default(),
        );
        let offer = &test_offer();

        // Cannot calculate price before bidding start
        assert!(market.price_at_block(offer, 99).is_err());

        assert_eq!(market.price_at_block(offer, 100).unwrap(), ether("1"));

        assert_eq!(market.price_at_block(offer, 101).unwrap(), ether("1.01"));
        assert_eq!(market.price_at_block(offer, 125).unwrap(), ether("1.25"));
        assert_eq!(market.price_at_block(offer, 150).unwrap(), ether("1.5"));
        assert_eq!(market.price_at_block(offer, 175).unwrap(), ether("1.75"));
        assert_eq!(market.price_at_block(offer, 199).unwrap(), ether("1.99"));

        assert_eq!(market.price_at_block(offer, 200).unwrap(), ether("2"));
        assert_eq!(market.price_at_block(offer, 500).unwrap(), ether("2"));
    }

    #[test]
    fn test_block_at_price() {
        let market = BoundlessMarketService::new(
            Address::default(),
            ProviderBuilder::default().on_http(Url::from_str("http://rpc.null").unwrap()),
            Address::default(),
        );
        let offer = &test_offer();

        assert_eq!(market.block_at_price(offer, ether("1")).unwrap(), 0);

        assert_eq!(market.block_at_price(offer, ether("1.01")).unwrap(), 101);
        assert_eq!(market.block_at_price(offer, ether("1.001")).unwrap(), 101);

        assert_eq!(market.block_at_price(offer, ether("1.25")).unwrap(), 125);
        assert_eq!(market.block_at_price(offer, ether("1.5")).unwrap(), 150);
        assert_eq!(market.block_at_price(offer, ether("1.75")).unwrap(), 175);
        assert_eq!(market.block_at_price(offer, ether("1.99")).unwrap(), 199);
        assert_eq!(market.block_at_price(offer, ether("2")).unwrap(), 200);

        // Price cannot exceed maxPrice
        assert!(market.block_at_price(offer, ether("3")).is_err());
    }

    #[tokio::test]
    async fn test_deposit_withdraw() {
        // Setup anvil
        let anvil = Anvil::new().spawn();

        let ctx =
            TestCtx::new(&anvil, Digest::from(SET_BUILDER_ID), Digest::from(ASSESSOR_GUEST_ID))
                .await
                .unwrap();

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
    async fn test_deposit_withdraw_stake() {
        // Setup anvil
        let anvil = Anvil::new().spawn();

        let ctx =
            TestCtx::new(&anvil, Digest::from(SET_BUILDER_ID), Digest::from(ASSESSOR_GUEST_ID))
                .await
                .unwrap();

        let deposit = U256::from(10);

        // Approve and deposit stake
        ctx.prover_market.approve_deposit_stake(deposit).await.unwrap();
        ctx.prover_market.deposit_stake(deposit).await.unwrap();

        // Deposit stake with permit
        ctx.prover_market.deposit_stake_with_permit(deposit, &ctx.prover_signer).await.unwrap();

        assert_eq!(
            ctx.prover_market.balance_of_stake(ctx.prover_signer.address()).await.unwrap(),
            U256::from(20)
        );

        // Withdraw prover balances
        ctx.prover_market.withdraw_stake(U256::from(20)).await.unwrap();
        assert_eq!(
            ctx.prover_market.balance_of_stake(ctx.prover_signer.address()).await.unwrap(),
            U256::ZERO
        );

        // Withdraw when balance is zero
        assert!(ctx.prover_market.withdraw_stake(U256::from(20)).await.is_err());
    }

    #[tokio::test]
    async fn test_submit_request() {
        // Setup anvil
        let anvil = Anvil::new().spawn();

        let ctx =
            TestCtx::new(&anvil, Digest::from(SET_BUILDER_ID), Digest::from(ASSESSOR_GUEST_ID))
                .await
                .unwrap();

        let request = new_request(1, &ctx).await;

        let request_id =
            ctx.customer_market.submit_request(&request, &ctx.customer_signer).await.unwrap();

        // fetch logs and check if the event was emitted
        let logs = ctx.customer_market.instance().RequestSubmitted_filter().query().await.unwrap();

        let (_, log) = logs.first().unwrap();
        let log = log.log_decode::<IBoundlessMarket::RequestSubmitted>().unwrap();
        assert!(log.inner.data.request.id == request_id);
    }

    #[tokio::test]
    async fn test_e2e() {
        // Setup anvil
        let anvil = Anvil::new().spawn();

        let ctx =
            TestCtx::new(&anvil, Digest::from(SET_BUILDER_ID), Digest::from(ASSESSOR_GUEST_ID))
                .await
                .unwrap();

        let eip712_domain = eip712_domain! {
            name: "IBoundlessMarket",
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
        let log = log.log_decode::<IBoundlessMarket::RequestSubmitted>().unwrap();
        let request = log.inner.data.request;
        let customer_sig = log.inner.data.clientSignature;

        // Deposit prover balances
        let deposit = default_allowance();
        ctx.prover_market.deposit_stake_with_permit(deposit, &ctx.prover_signer).await.unwrap();

        // Lockin the request
        ctx.prover_market.lockin_request(&request, &customer_sig, None).await.unwrap();
        assert!(ctx.customer_market.is_locked_in(request_id).await.unwrap());
        assert!(
            ctx.customer_market.get_status(request_id, Some(expires_at)).await.unwrap()
                == ProofStatus::Locked
        );

        // mock the fulfillment
        let (root, set_verifier_seal, fulfillment, assessor_seal) =
            mock_singleton(&request, eip712_domain, ctx.prover_signer.address());

        // publish the committed root
        ctx.set_verifier.submit_merkle_root(root, set_verifier_seal).await.unwrap();

        // fulfill the request
        ctx.prover_market
            .fulfill(&fulfillment, &assessor_seal, ctx.prover_signer.address())
            .await
            .unwrap();
        assert!(ctx.customer_market.is_fulfilled(request_id).await.unwrap());

        // retrieve journal and seal from the fulfilled request
        let (journal, seal) =
            ctx.customer_market.get_request_fulfillment(request_id).await.unwrap();

        assert_eq!(journal, fulfillment.journal);
        assert_eq!(seal, fulfillment.seal);
    }

    #[tokio::test]
    async fn test_e2e_merged_submit_fulfill() {
        // Setup anvil
        let anvil = Anvil::new().spawn();

        let ctx =
            TestCtx::new(&anvil, Digest::from(SET_BUILDER_ID), Digest::from(ASSESSOR_GUEST_ID))
                .await
                .unwrap();

        let eip712_domain = eip712_domain! {
            name: "IBoundlessMarket",
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
        let log = log.log_decode::<IBoundlessMarket::RequestSubmitted>().unwrap();
        let request = log.inner.data.request;
        let customer_sig = log.inner.data.clientSignature;

        // Deposit prover balances
        let deposit = default_allowance();
        ctx.prover_market.deposit_stake_with_permit(deposit, &ctx.prover_signer).await.unwrap();

        // Lockin the request
        ctx.prover_market.lockin_request(&request, &customer_sig, None).await.unwrap();
        assert!(ctx.customer_market.is_locked_in(request_id).await.unwrap());
        assert!(
            ctx.customer_market.get_status(request_id, Some(expires_at)).await.unwrap()
                == ProofStatus::Locked
        );

        // mock the fulfillment
        let (root, set_verifier_seal, fulfillment, assessor_seal) =
            mock_singleton(&request, eip712_domain, ctx.prover_signer.address());

        let fulfillments = vec![fulfillment];
        // publish the committed root + fulfillments
        ctx.prover_market
            .submit_merkle_and_fulfill(
                root,
                set_verifier_seal,
                fulfillments.clone(),
                assessor_seal,
                ctx.prover_signer.address(),
            )
            .await
            .unwrap();

        // retrieve journal and seal from the fulfilled request
        let (journal, seal) =
            ctx.customer_market.get_request_fulfillment(request_id).await.unwrap();

        assert_eq!(journal, fulfillments[0].journal);
        assert_eq!(seal, fulfillments[0].seal);
    }

    #[tokio::test]
    async fn test_e2e_price_and_fulfill_batch() {
        // Setup anvil
        let anvil = Anvil::new().spawn();

        let ctx =
            TestCtx::new(&anvil, Digest::from(SET_BUILDER_ID), Digest::from(ASSESSOR_GUEST_ID))
                .await
                .unwrap();

        let eip712_domain = eip712_domain! {
            name: "IBoundlessMarket",
            version: "1",
            chain_id: anvil.chain_id(),
            verifying_contract: *ctx.customer_market.instance().address(),
        };

        let request = new_request(1, &ctx).await;
        let request_id =
            ctx.customer_market.submit_request(&request, &ctx.customer_signer).await.unwrap();

        // fetch logs to retrieve the customer signature from the event
        let logs = ctx.customer_market.instance().RequestSubmitted_filter().query().await.unwrap();

        let (_, log) = logs.first().unwrap();
        let log = log.log_decode::<IBoundlessMarket::RequestSubmitted>().unwrap();
        let request = log.inner.data.request;
        let customer_sig = log.inner.data.clientSignature;

        // mock the fulfillment
        let (root, set_verifier_seal, fulfillment, assessor_seal) =
            mock_singleton(&request, eip712_domain, ctx.prover_signer.address());

        let fulfillments = vec![fulfillment];

        // publish the committed root
        ctx.set_verifier.submit_merkle_root(root, set_verifier_seal).await.unwrap();

        // Price and fulfill the request
        ctx.prover_market
            .price_and_fulfill_batch(
                vec![request],
                vec![customer_sig],
                fulfillments.clone(),
                assessor_seal,
                ctx.prover_signer.address(),
                None,
            )
            .await
            .unwrap();

        // retrieve journal and seal from the fulfilled request
        let (journal, seal) =
            ctx.customer_market.get_request_fulfillment(request_id).await.unwrap();

        assert_eq!(journal, fulfillments[0].journal);
        assert_eq!(seal, fulfillments[0].seal);
    }

    #[tokio::test]
    async fn test_e2e_payment_failed() {
        // Setup anvil
        let anvil = Anvil::new().spawn();

        let ctx =
            TestCtx::new(&anvil, Digest::from(SET_BUILDER_ID), Digest::from(ASSESSOR_GUEST_ID))
                .await
                .unwrap();

        let eip712_domain = eip712_domain! {
            name: "IBoundlessMarket",
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
        let log = log.log_decode::<IBoundlessMarket::RequestSubmitted>().unwrap();
        let request = log.inner.data.request;
        let customer_sig = log.inner.data.clientSignature;

        // Deposit prover balances
        let deposit = default_allowance();
        ctx.prover_market.deposit_stake_with_permit(deposit, &ctx.prover_signer).await.unwrap();

        // Lockin the request
        ctx.prover_market.lockin_request(&request, &customer_sig, None).await.unwrap();
        assert!(ctx.customer_market.is_locked_in(request_id).await.unwrap());
        assert!(
            ctx.customer_market.get_status(request_id, Some(expires_at)).await.unwrap()
                == ProofStatus::Locked
        );

        // Test behavior when payment requirements are not met.
        {
            // mock the fulfillment, using the wrong prover address. Address::from(3) arbitrary.
            let some_other_address = Address::from(U160::from(3));
            let (root, set_verifier_seal, fulfillment, assessor_seal) =
                mock_singleton(&request, eip712_domain.clone(), some_other_address);

            // publish the committed root
            ctx.set_verifier.submit_merkle_root(root, set_verifier_seal).await.unwrap();

            // attempt to fulfill the request, and ensure we revert.
            ctx.prover_market
                .fulfill(&fulfillment, &assessor_seal, some_other_address)
                .await
                .unwrap_err(); // TODO: Use the error
            assert!(!ctx.customer_market.is_fulfilled(request_id).await.unwrap());

            let mut fulfillment_no_payment = fulfillment;
            fulfillment_no_payment.requirePayment = false;

            // attempt to fulfill the request, and ensure we revert.
            let log = ctx
                .prover_market
                .fulfill(&fulfillment_no_payment, &assessor_seal, some_other_address)
                .await
                .unwrap();

            assert!(ctx.customer_market.is_fulfilled(request_id).await.unwrap());
            // TODO: Decode the log and assert on the particular error.
            assert!(log.is_some());

            // retrieve journal and seal from the fulfilled request
            let (journal, seal) =
                ctx.customer_market.get_request_fulfillment(request_id).await.unwrap();

            assert_eq!(journal, fulfillment_no_payment.journal);
            assert_eq!(seal, fulfillment_no_payment.seal);
        }

        // mock the fulfillment, this time using the right prover address.
        let (root, set_verifier_seal, fulfillment, assessor_seal) =
            mock_singleton(&request, eip712_domain, ctx.prover_signer.address());

        // publish the committed root
        ctx.set_verifier.submit_merkle_root(root, set_verifier_seal).await.unwrap();

        // fulfill the request, this time getting paid.
        let log = ctx
            .prover_market
            .fulfill(&fulfillment, &assessor_seal, ctx.prover_signer.address())
            .await
            .unwrap();
        assert!(ctx.customer_market.is_fulfilled(request_id).await.unwrap());
        assert!(log.is_none());

        // retrieve journal and seal from the fulfilled request
        let (_journal, _seal) =
            ctx.customer_market.get_request_fulfillment(request_id).await.unwrap();

        // TODO: Instead of checking that this is the same seal, check if this is some valid seal.
        // When there are multiple fulfillments one order, there will be multiple ProofDelivered
        // events. All proofs will be valid though.
        //assert_eq!(journal, fulfillment.journal);
        //assert_eq!(seal, fulfillment.seal);
    }
}
