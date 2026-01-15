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

use std::{
    fmt::Debug,
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};

use alloy::{
    consensus::{BlockHeader, Transaction},
    eips::BlockNumberOrTag,
    network::Ethereum,
    primitives::{utils::format_ether, Address, Bytes, B256, U256},
    providers::{PendingTransactionBuilder, PendingTransactionError, Provider},
    rpc::types::{Log, TransactionReceipt},
    signers::Signer,
};

use alloy_sol_types::{SolCall, SolEvent, SolInterface};
use anyhow::{anyhow, Context, Result};
use thiserror::Error;

use super::{
    eip712_domain, AssessorReceipt, EIP712DomainSaltless, Fulfillment,
    IBoundlessMarket::{self, IBoundlessMarketErrors, IBoundlessMarketInstance, ProofDelivered},
    Offer, ProofRequest, RequestError, RequestId, RequestStatus, TxnErr, TXN_CONFIRM_TIMEOUT,
};
use crate::{
    contracts::token::{IERC20Permit, IHitPoints::IHitPointsErrors, Permit, IERC20},
    deployments::collateral_token_supports_permit,
};

/// Retry configuration for query operations.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct EventRetryConfig {
    /// Number of retries to attempt if a query fails.
    ///
    /// Retrying queries can be useful to handle RPC provider data inconsistency issues
    /// where calls to fetch events or current block numbers return stale data. For example,
    /// if an event was just emitted but the RPC provider hasn't indexed it yet, a retry
    /// after a short delay may succeed.
    pub retries: u32,
    /// Fixed delay in milliseconds between retry attempts.
    ///
    /// This delay helps handle transient RPC provider issues where data may be temporarily
    /// inconsistent or unavailable. A short delay (e.g., 5000ms) is typically sufficient
    /// to allow the RPC provider to catch up with the latest blockchain state.
    ///
    /// If `None`, no delay is used between retries.
    pub retry_delay_ms: Option<u64>,
}

/// Event query configuration for searching blockchain events.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct EventQueryConfig {
    /// Number of blocks to query per iteration.
    pub block_range: u64,
    /// Maximum number of iterations to search through blocks.
    pub max_iterations: u64,
    /// Retry configuration for query operations.
    ///
    /// If `None`, no retries are attempted.
    pub retry_config: Option<EventRetryConfig>,
}

impl Default for EventQueryConfig {
    /// Default configuration aimed at SDK operations.
    ///
    /// This configuration is suitable for SDK operations that require some resilience
    /// against transient RPC provider issues. The default block_range is 1000 blocks,
    /// which is a reasonable compromise between search depth and RPC provider performance.
    /// The default enables searching back ~350,000 blocks, which is around 1 week on Base Mainnet.
    fn default() -> Self {
        Self {
            block_range: 499,
            max_iterations: 500,
            retry_config: Some(EventRetryConfig { retries: 1, retry_delay_ms: Some(5000) }),
        }
    }
}

impl EventQueryConfig {
    /// Default configuration for broker operations: no retries.
    ///
    /// Broker operations disable retries to prevent delays, search fewer blocks as the broker tends
    /// to only query in-flight orders, and uses a block_range config aimed to support a wide range  of
    /// free tier RPCs.
    ///
    /// Default config enables searching back ~50,000 blocks, which is around 1 day on Base Mainnet.
    pub fn default_for_broker() -> Self {
        Self { block_range: 499, max_iterations: 100, retry_config: None }
    }
}

/// Fraction of collateral the protocol gives to the prover who fills an order that was locked by another prover but expired
/// This is determined by the constant SLASHING_BURN_BPS defined in the BoundlessMarket contract.
/// The value is 4 because the slashing burn is 75% of the collateral, and we give the remaining 1/4 of that to the prover.
/// TODO(https://github.com/boundless-xyz/boundless/issues/517): Retrieve this from the contract in the future
const FRACTION_STAKE_NUMERATOR: u64 = 1;
const FRACTION_STAKE_DENOMINATOR: u64 = 2;

/// Boundless market errors.
#[derive(Error, Debug)]
pub enum MarketError {
    /// Transaction error.
    #[error("Transaction error: {0}")]
    TxnError(#[from] TxnErr),

    /// Transaction confirmation error.
    #[error("Transaction confirmation error: {0:?}")]
    TxnConfirmationError(anyhow::Error),

    /// Request not fulfilled.
    #[error("Request is not fulfilled 0x{0:x}")]
    RequestNotFulfilled(U256),

    /// Request has expired.
    #[error("Request has expired 0x{0:x}")]
    RequestHasExpired(U256),

    /// Request already slashed.
    #[error("Request is slashed 0x{0:x}")]
    RequestIsSlashed(U256),

    /// Request malformed.
    #[error("Request error {0}")]
    RequestError(#[from] RequestError),

    /// Request address does not match with signer.
    #[error("Request address does not match with signer {0} - {0}")]
    AddressMismatch(Address, Address),

    /// Proof not found.
    #[error("Proof not found for request 0x{0:x} in events logs after searching from block {1} to block {2}. Consider increasing EventQueryConfig.block_range or EventQueryConfig.max_iterations")]
    ProofNotFound(U256, u64, u64),

    /// Request not found.
    #[error("Request not found in event logs 0x{0:x}")]
    RequestNotFound(U256),

    /// Request already locked.
    #[error("Request already locked: 0x{0:x}")]
    RequestAlreadyLocked(U256),

    /// Lock request reverted, possibly outbid.
    #[error("Lock request reverted, possibly outbid: txn_hash: {0}")]
    LockRevert(B256),

    /// Lock request reverted, possibly outbid.
    #[error("Slash request reverted, possibly already slashed: txn_hash: {0}")]
    SlashRevert(B256),

    /// Log not emitted.
    #[error("Expected log was not emitted, txn {0} possibly reverted: {1}")]
    LogNotEmitted(B256, anyhow::Error),

    /// General market error.
    #[error("Other error: {0:?}")]
    Error(#[from] anyhow::Error),

    /// Timeout reached.
    #[error("Timeout: 0x{0:x}")]
    TimeoutReached(U256),

    /// Payment requirements failed
    #[error("Payment requirements failed during order fulfillment: {0:?}")]
    PaymentRequirementsFailed(IBoundlessMarketErrors),

    /// Payment requirements failed, unable to decode error
    #[error(
        "Payment requirements failed during order fulfillment: unrecognized error payload {0:?}"
    )]
    PaymentRequirementsFailedUnknownError(Bytes),
}

impl From<alloy::contract::Error> for MarketError {
    fn from(err: alloy::contract::Error) -> Self {
        tracing::debug!("raw alloy contract error: {:?}", err);
        MarketError::Error(TxnErr::from(err).into())
    }
}

/// Proof market service.
pub struct BoundlessMarketService<P> {
    instance: IBoundlessMarketInstance<P, Ethereum>,
    // Chain ID with caching to ensure we fetch it at most once.
    // Note: always access via get_chain_id()
    chain_id: AtomicU64,
    caller: Address,
    timeout: Duration,
    event_query_config: EventQueryConfig,
    balance_alert_config: StakeBalanceAlertConfig,
    receipt_query_config: ReceiptQueryConfig,
}

#[derive(Clone, Debug)]
struct ReceiptQueryConfig {
    /// Interval at which the transaction receipts are polled.
    retry_interval: Duration,
    /// Number of retries for querying receipt of lock transactions.
    retry_count: usize,
}

impl Default for ReceiptQueryConfig {
    fn default() -> Self {
        Self { retry_count: 10, retry_interval: Duration::from_millis(500) }
    }
}

#[derive(Clone, Debug, Default)]
struct StakeBalanceAlertConfig {
    /// Threshold at which to log a warning
    warn_threshold: Option<U256>,
    /// Threshold at which to log an error
    error_threshold: Option<U256>,
}

impl<P: Clone> Clone for BoundlessMarketService<P> {
    fn clone(&self) -> Self {
        Self {
            instance: self.instance.clone(),
            chain_id: self.chain_id.load(Ordering::Relaxed).into(),
            caller: self.caller,
            timeout: self.timeout,
            event_query_config: self.event_query_config.clone(),
            balance_alert_config: self.balance_alert_config.clone(),
            receipt_query_config: self.receipt_query_config.clone(),
        }
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
        [] => Err(anyhow!(
            "transaction 0x{:x} did not emit event {}",
            receipt.transaction_hash,
            E::SIGNATURE
        )),
        _ => Err(anyhow!(
            "transaction emitted more than one event with signature {}, {:#?}",
            E::SIGNATURE,
            logs
        )),
    }
}

fn validate_fulfill_receipt(receipt: TransactionReceipt) -> Result<(), MarketError> {
    for (idx, log) in receipt.inner.logs().iter().enumerate() {
        if log.topic0().is_some_and(|topic| {
            *topic == IBoundlessMarket::PaymentRequirementsFailed::SIGNATURE_HASH
        }) {
            match log.log_decode::<IBoundlessMarket::PaymentRequirementsFailed>() {
                Ok(decoded) => {
                    let raw_error = Bytes::copy_from_slice(decoded.inner.data.error.as_ref());
                    match IBoundlessMarketErrors::abi_decode(&raw_error) {
                        Ok(err) => tracing::warn!(
                            tx_hash = ?receipt.transaction_hash,
                            log_index = idx,
                            "Payment requirements failed for at least one fulfillment: {err:?}"
                        ),
                        Err(_) => tracing::warn!(
                            tx_hash = ?receipt.transaction_hash,
                            log_index = idx,
                            raw = ?raw_error,
                            "Payment requirements failed for at least one fulfillment, but error payload was unrecognized"
                        ),
                    }
                }
                Err(err) => tracing::warn!(
                    tx_hash = ?receipt.transaction_hash,
                    log_index = idx,
                    "Failed to decode PaymentRequirementsFailed event: {err:?}"
                ),
            }
        }
    }
    Ok(())
}

/// Data returned when querying for a RequestSubmitted event
#[derive(Debug, Clone)]
pub struct RequestSubmittedEventData {
    /// The proof request
    pub request: ProofRequest,
    /// The client signature
    pub client_signature: Bytes,
    /// The block number where the event occurred
    pub block_number: u64,
    /// The transaction hash containing the event
    pub tx_hash: B256,
}

/// Data returned when querying for a RequestLocked event
#[derive(Debug, Clone)]
pub struct RequestLockedEventData {
    /// The RequestLocked event data
    pub event: IBoundlessMarket::RequestLocked,
    /// The block number where the event occurred
    pub block_number: u64,
    /// The transaction hash containing the event
    pub tx_hash: B256,
}

/// Data returned when querying for a RequestFulfilled event
#[derive(Debug, Clone)]
pub struct RequestFulfilledEventData {
    /// The RequestFulfilled event data
    pub event: IBoundlessMarket::RequestFulfilled,
    /// The block number where the event occurred
    pub block_number: u64,
    /// The transaction hash containing the event
    pub tx_hash: B256,
}

/// Data returned when querying for ProofDelivered events
#[derive(Debug, Clone)]
pub struct ProofDeliveredEventData {
    /// The ProofDelivered event data
    pub event: ProofDelivered,
    /// The block number where the event occurred
    pub block_number: u64,
    /// The transaction hash containing the event
    pub tx_hash: B256,
}

/// Data returned when querying for a ProverSlashed event
#[derive(Debug, Clone)]
pub struct ProverSlashedEventData {
    /// The ProverSlashed event data
    pub event: IBoundlessMarket::ProverSlashed,
    /// The block number where the event occurred
    pub block_number: u64,
    /// The transaction hash containing the event
    pub tx_hash: B256,
}

impl<P: Provider> BoundlessMarketService<P> {
    /// Creates a new Boundless market service.
    ///
    /// The default configuration includes retry logic when querying events to handle
    /// transient RPC provider issues.
    pub fn new(address: impl Into<Address>, provider: P, caller: impl Into<Address>) -> Self {
        let instance = IBoundlessMarket::new(address.into(), provider);

        Self {
            instance,
            chain_id: AtomicU64::new(0),
            caller: caller.into(),
            timeout: TXN_CONFIRM_TIMEOUT,
            event_query_config: EventQueryConfig::default(),
            balance_alert_config: StakeBalanceAlertConfig::default(),
            receipt_query_config: ReceiptQueryConfig::default(),
        }
    }

    /// Creates a new `BoundlessMarketService` instance optimized for broker operations.
    ///
    /// This function initializes the service with `EventQueryConfig` set to disable retries.
    /// The broker handles retries at a higher level through its own retry mechanisms.
    pub fn new_for_broker(
        address: impl Into<Address>,
        provider: P,
        caller: impl Into<Address>,
    ) -> Self {
        let instance = IBoundlessMarket::new(address.into(), provider);

        Self {
            instance,
            chain_id: AtomicU64::new(0),
            caller: caller.into(),
            timeout: TXN_CONFIRM_TIMEOUT,
            event_query_config: EventQueryConfig::default_for_broker(),
            balance_alert_config: StakeBalanceAlertConfig::default(),
            receipt_query_config: ReceiptQueryConfig::default(),
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

    /// Set collateral balance thresholds to warn or error alert on
    pub fn with_collateral_balance_alert(
        self,
        warn_threshold: &Option<U256>,
        error_threshold: &Option<U256>,
    ) -> Self {
        Self {
            balance_alert_config: StakeBalanceAlertConfig {
                warn_threshold: *warn_threshold,
                error_threshold: *error_threshold,
            },
            ..self
        }
    }

    /// Retry count for confirmed transactions receipts.
    pub fn with_receipt_retry_count(mut self, count: usize) -> Self {
        self.receipt_query_config.retry_count = count;
        self
    }

    /// Retry polling interval for confirmed transactions receipts.
    pub fn with_receipt_retry_interval(mut self, interval: Duration) -> Self {
        self.receipt_query_config.retry_interval = interval;
        self
    }

    /// Returns the market contract instance.
    pub fn instance(&self) -> &IBoundlessMarketInstance<P, Ethereum> {
        &self.instance
    }

    /// Returns the caller address.
    pub fn caller(&self) -> Address {
        self.caller
    }

    /// Get the EIP-712 domain associated with the market contract.
    ///
    /// If not cached, this function will fetch the chain ID with an RPC call.
    pub async fn eip712_domain(&self) -> Result<EIP712DomainSaltless, MarketError> {
        Ok(eip712_domain(*self.instance.address(), self.get_chain_id().await?))
    }

    /// Deposit Ether into the market to pay for proof and/or lockin collateral.
    pub async fn deposit(&self, value: U256) -> Result<(), MarketError> {
        tracing::trace!("Calling deposit() value: {value}");
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
        tracing::trace!("Calling withdraw({amount})");
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

    /// Returns the balance, in Wei, of the given account.
    pub async fn balance_of(&self, account: impl Into<Address>) -> Result<U256, MarketError> {
        let account = account.into();
        tracing::trace!("Calling balanceOf({account})");
        let balance = self.instance.balanceOf(account).call().await?;

        Ok(balance)
    }

    /// Submit a request such that it is publicly available for provers to evaluate and bid
    /// on. Includes the specified value, which will be deposited to the account of msg.sender.
    pub async fn submit_request_with_value(
        &self,
        request: &ProofRequest,
        signer: &impl Signer,
        value: impl Into<U256>,
    ) -> Result<U256, MarketError> {
        tracing::trace!("Calling submitRequest({:x?})", request);
        tracing::debug!("Sending request ID {:x}", request.id);
        let client_address = request.client_address();
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
        tracing::debug!(
            "Broadcasting tx {:x} with request ID {:x}",
            pending_tx.tx_hash(),
            request.id
        );

        let receipt = self.get_receipt_with_retry(pending_tx).await?;

        // Look for the logs for submitting the transaction.
        match extract_tx_log::<IBoundlessMarket::RequestSubmitted>(&receipt) {
            Ok(log) => Ok(U256::from(log.inner.data.requestId)),
            Err(e) => Err(MarketError::LogNotEmitted(receipt.transaction_hash, e)),
        }
    }

    /// Submit a request such that it is publicly available for provers to evaluate and bid
    /// on, with a signature specified as Bytes.
    pub async fn submit_request_with_signature(
        &self,
        request: &ProofRequest,
        signature: impl Into<Bytes>,
    ) -> Result<U256, MarketError> {
        tracing::trace!("Calling submitRequest({:x?})", request);
        tracing::debug!("Sending request ID {:x}", request.id);
        let call = self.instance.submitRequest(request.clone(), signature.into()).from(self.caller);
        let pending_tx = call.send().await?;
        tracing::debug!(
            "Broadcasting tx {:x} with request ID {:x}",
            pending_tx.tx_hash(),
            request.id
        );

        let receipt = self.get_receipt_with_retry(pending_tx).await?;

        // Look for the logs for submitting the transaction.
        match extract_tx_log::<IBoundlessMarket::RequestSubmitted>(&receipt) {
            Ok(log) => Ok(U256::from(log.inner.data.requestId)),
            Err(e) => Err(MarketError::LogNotEmitted(receipt.transaction_hash, e)),
        }
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
        if value > U256::ZERO {
            tracing::debug!("Sending {} ETH with request {:x}", format_ether(value), request.id);
        }
        self.submit_request_with_value(request, signer, value).await
    }

    /// Lock the request to the prover, giving them exclusive rights to be paid to
    /// fulfill this request, and also making them subject to slashing penalties if they fail to
    /// deliver. At this point, the price for fulfillment is also set, based on the reverse Dutch
    /// auction parameters and the block number at which this transaction is processed.
    ///
    /// This method should be called from the address of the prover.
    pub async fn lock_request(
        &self,
        request: &ProofRequest,
        client_sig: impl Into<Bytes>,
    ) -> Result<u64, MarketError> {
        tracing::trace!("Calling requestIsLocked({:x})", request.id);
        let is_locked_in: bool =
            self.instance.requestIsLocked(request.id).call().await.context("call failed")?;
        if is_locked_in {
            return Err(MarketError::RequestAlreadyLocked(request.id));
        }

        let client_sig_bytes = client_sig.into();
        tracing::trace!("Calling lockRequest({:x?}, {:x?})", request, client_sig_bytes);

        let call = self.instance.lockRequest(request.clone(), client_sig_bytes).from(self.caller);

        tracing::trace!("Sending tx {}", format!("{:?}", call));
        let pending_tx = call.send().await?;

        let tx_hash = *pending_tx.tx_hash();
        tracing::trace!("Broadcasting lock request tx {}", tx_hash);

        let receipt = self.get_receipt_with_retry(pending_tx).await?;

        if !receipt.status() {
            // TODO: Get + print revertReason
            return Err(MarketError::LockRevert(receipt.transaction_hash));
        }

        tracing::info!(
            "Locked request {:x}, transaction hash: {}",
            request.id,
            receipt.transaction_hash
        );

        self.check_collateral_balance().await?;

        Ok(receipt.block_number.context("TXN Receipt missing block number")?)
    }

    /// Lock the request to the prover, giving them exclusive rights to be paid to
    /// fulfill this request, and also making them subject to slashing penalties if they fail to
    /// deliver. At this point, the price for fulfillment is also set, based on the reverse Dutch
    /// auction parameters and the block at which this transaction is processed.
    ///
    /// This method uses the provided signature to authenticate the prover. Note that the prover
    /// signature must be over the LockRequest struct, not the ProofRequest struct.
    pub async fn lock_request_with_signature(
        &self,
        request: &ProofRequest,
        client_sig: impl Into<Bytes>,
        prover_sig: impl Into<Bytes>,
        _priority_gas: Option<u128>,
    ) -> Result<u64, MarketError> {
        tracing::trace!("Calling requestIsLocked({:x})", request.id);
        let is_locked_in: bool =
            self.instance.requestIsLocked(request.id).call().await.context("call failed")?;
        if is_locked_in {
            return Err(MarketError::RequestAlreadyLocked(request.id));
        }

        let client_sig_bytes = client_sig.into();
        let prover_sig_bytes = prover_sig.into();
        tracing::trace!(
            "Calling lockRequestWithSignature({:x?}, {:x?}, {:x?})",
            request,
            client_sig_bytes,
            prover_sig_bytes
        );

        let call = self
            .instance
            .lockRequestWithSignature(request.clone(), client_sig_bytes.clone(), prover_sig_bytes)
            .from(self.caller);
        let pending_tx = call.send().await.context("Failed to lock")?;
        tracing::trace!("Broadcasting lock request with signature tx {}", pending_tx.tx_hash());

        let receipt = self.get_receipt_with_retry(pending_tx).await?;
        if !receipt.status() {
            // TODO: Get + print revertReason
            return Err(MarketError::LockRevert(receipt.transaction_hash));
        }

        tracing::info!(
            "Locked request {:x}, transaction hash: {}",
            request.id,
            receipt.transaction_hash
        );

        Ok(receipt.block_number.context("TXN Receipt missing block number")?)
    }

    async fn get_receipt_with_retry(
        &self,
        pending_tx: PendingTransactionBuilder<Ethereum>,
    ) -> Result<TransactionReceipt, MarketError> {
        let tx_hash = *pending_tx.tx_hash();

        // Get the nonce of the transaction for debugging purposes.
        // It is possible that the transaction is not found immediately after broadcast, so we don't error if it's not found.
        let tx_result = self.instance.provider().get_transaction_by_hash(tx_hash).await;
        if let Ok(Some(tx)) = tx_result {
            let nonce = tx.nonce();
            tracing::debug!("Tx {} broadcasted with nonce {}", tx_hash, nonce);
        } else {
            tracing::debug!(
                "Tx {} not found immediately after broadcast. Can't get nonce.",
                tx_hash
            );
        }

        match pending_tx.with_timeout(Some(self.timeout)).get_receipt().await {
            Ok(receipt) => Ok(receipt),
            Err(PendingTransactionError::TransportError(err)) if err.is_null_resp() => {
                tracing::debug!("failed to query receipt of confirmed transaction, retrying");
                // There is a race condition with some providers where a transaction will be
                // confirmed through the RPC, but querying the receipt returns null when requested
                // immediately after.
                for _ in 0..self.receipt_query_config.retry_count {
                    if let Ok(Some(receipt)) =
                        self.instance.provider().get_transaction_receipt(tx_hash).await
                    {
                        return Ok(receipt);
                    }

                    tokio::time::sleep(self.receipt_query_config.retry_interval).await;
                }

                Err(anyhow!(
                    "Transaction {:?} confirmed, but receipt was not found after {} retries.",
                    tx_hash,
                    self.receipt_query_config.retry_count
                )
                .into())
            }
            Err(e) => Err(MarketError::TxnConfirmationError(anyhow!(
                "failed to confirm tx {:?} within timeout {:?}: {}",
                tx_hash,
                self.timeout,
                e
            ))),
        }
    }

    /// When a prover fails to fulfill a request by the deadline, this function can be used to burn
    /// the associated prover collateral.
    pub async fn slash(
        &self,
        request_id: U256,
    ) -> Result<IBoundlessMarket::ProverSlashed, MarketError> {
        if self.is_slashed(request_id).await? {
            return Err(MarketError::RequestIsSlashed(request_id));
        }

        tracing::trace!("Calling slash({:x?})", request_id);
        let call = self.instance.slash(request_id).from(self.caller);
        let pending_tx = call.send().await?;
        tracing::debug!("Broadcasting tx {}", pending_tx.tx_hash());

        let receipt = self.get_receipt_with_retry(pending_tx).await?;

        if !receipt.status() {
            return Err(MarketError::SlashRevert(receipt.transaction_hash));
        }

        match extract_tx_log::<IBoundlessMarket::ProverSlashed>(&receipt) {
            Ok(log) => Ok(log.inner.data),
            Err(e) => Err(MarketError::LogNotEmitted(receipt.transaction_hash, e)),
        }
    }

    /// Submits a `FulfillmentTx`.
    pub async fn fulfill(&self, tx: FulfillmentTx) -> Result<(), MarketError> {
        let FulfillmentTx { root, unlocked_requests, fulfillments, assessor_receipt, withdraw } =
            tx;
        let price = !unlocked_requests.is_empty();
        let request_ids = fulfillments.iter().map(|fill| fill.id).collect::<Vec<_>>();

        match root {
            None => match (price, withdraw) {
                (false, false) => {
                    tracing::debug!("Fulfilling requests {:?} with fulfill", request_ids);
                    self._fulfill(fulfillments, assessor_receipt).await
                }
                (false, true) => {
                    tracing::debug!(
                        "Fulfilling requests {:?} with fulfill and withdraw",
                        request_ids
                    );
                    self.fulfill_and_withdraw(fulfillments, assessor_receipt).await
                }
                (true, false) => {
                    tracing::debug!("Fulfilling requests {:?} with price and fulfill", request_ids);
                    self.price_and_fulfill(unlocked_requests, fulfillments, assessor_receipt).await
                }
                (true, true) => {
                    tracing::debug!(
                        "Fulfilling requests {:?} with price and fulfill and withdraw",
                        request_ids
                    );
                    self.price_and_fulfill_and_withdraw(
                        unlocked_requests,
                        fulfillments,
                        assessor_receipt,
                    )
                    .await
                }
            },
            Some(root) => match (price, withdraw) {
                (false, false) => {
                    tracing::debug!(
                        "Fulfilling requests {:?} with submitting root and fulfill",
                        request_ids
                    );
                    self.submit_root_and_fulfill(root, fulfillments, assessor_receipt).await
                }
                (false, true) => {
                    tracing::debug!(
                        "Fulfilling requests {:?} with submitting root and fulfill and withdraw",
                        request_ids
                    );
                    self.submit_root_and_fulfill_and_withdraw(root, fulfillments, assessor_receipt)
                        .await
                }
                (true, false) => {
                    tracing::debug!(
                        "Fulfilling requests {:?} with submitting root and price and fulfill",
                        request_ids
                    );
                    self.submit_root_and_price_fulfill(
                        root,
                        unlocked_requests,
                        fulfillments,
                        assessor_receipt,
                    )
                    .await
                }
                (true, true) => {
                    tracing::debug!("Fulfilling requests {:?} with submitting root and price and fulfill and withdraw", request_ids);
                    self.submit_root_and_price_fulfill_and_withdraw(
                        root,
                        unlocked_requests,
                        fulfillments,
                        assessor_receipt,
                    )
                    .await
                }
            },
        }
    }

    /// Fulfill a batch of requests by delivering the proof for each application.
    ///
    /// See [BoundlessMarketService::fulfill] for more details.
    async fn _fulfill(
        &self,
        fulfillments: Vec<Fulfillment>,
        assessor_fill: AssessorReceipt,
    ) -> Result<(), MarketError> {
        let fill_ids = fulfillments.iter().map(|fill| fill.id).collect::<Vec<_>>();
        tracing::trace!("Calling fulfill({fulfillments:?}, {assessor_fill:?})");
        let call = self.instance.fulfill(fulfillments, assessor_fill).from(self.caller);
        tracing::trace!("Calldata: {:x}", call.calldata());
        let pending_tx = call.send().await?;
        tracing::debug!("Broadcasting tx {}", pending_tx.tx_hash());

        let receipt = self.get_receipt_with_retry(pending_tx).await?;

        tracing::info!("Submitted proof for batch {:?}: {}", fill_ids, receipt.transaction_hash);

        validate_fulfill_receipt(receipt)
    }

    /// Fulfill a batch of requests by delivering the proof for each application and withdraw from the prover balance.
    ///
    /// See [BoundlessMarketService::fulfill] for more details.
    async fn fulfill_and_withdraw(
        &self,
        fulfillments: Vec<Fulfillment>,
        assessor_fill: AssessorReceipt,
    ) -> Result<(), MarketError> {
        let fill_ids = fulfillments.iter().map(|fill| fill.id).collect::<Vec<_>>();
        tracing::trace!("Calling fulfillAndWithdraw({fulfillments:?}, {assessor_fill:?})");
        let call = self.instance.fulfillAndWithdraw(fulfillments, assessor_fill).from(self.caller);
        tracing::trace!("Calldata: {:x}", call.calldata());
        let pending_tx = call.send().await?;
        tracing::debug!("Broadcasting tx {}", pending_tx.tx_hash());

        let receipt = self.get_receipt_with_retry(pending_tx).await?;

        tracing::info!("Submitted proof for batch {:?}: {}", fill_ids, receipt.transaction_hash);

        validate_fulfill_receipt(receipt)
    }

    /// Combined function to submit a new merkle root to the set-verifier and call `fulfill`.
    /// Useful to reduce the transaction count for fulfillments
    async fn submit_root_and_fulfill(
        &self,
        root: Root,
        fulfillments: Vec<Fulfillment>,
        assessor_fill: AssessorReceipt,
    ) -> Result<(), MarketError> {
        tracing::trace!(
            "Calling submitRootAndFulfill({:?}, {:x}, {fulfillments:?}, {assessor_fill:?})",
            root.root,
            root.seal
        );
        let call = self
            .instance
            .submitRootAndFulfill(
                root.verifier_address,
                root.root,
                root.seal,
                fulfillments,
                assessor_fill,
            )
            .from(self.caller);
        tracing::trace!("Calldata: {}", call.calldata());
        let pending_tx = call.send().await?;
        tracing::debug!("Broadcasting tx {}", pending_tx.tx_hash());
        let tx_receipt = self.get_receipt_with_retry(pending_tx).await?;

        tracing::info!("Submitted merkle root and proof for batch {}", tx_receipt.transaction_hash);

        validate_fulfill_receipt(tx_receipt)
    }

    /// Combined function to submit a new merkle root to the set-verifier and call `fulfillAndWithdraw`.
    /// Useful to reduce the transaction count for fulfillments
    async fn submit_root_and_fulfill_and_withdraw(
        &self,
        root: Root,
        fulfillments: Vec<Fulfillment>,
        assessor_fill: AssessorReceipt,
    ) -> Result<(), MarketError> {
        tracing::trace!("Calling submitRootAndFulfillAndWithdraw({:?}, {:x}, {fulfillments:?}, {assessor_fill:?})", root.root, root.seal);
        let call = self
            .instance
            .submitRootAndFulfillAndWithdraw(
                root.verifier_address,
                root.root,
                root.seal,
                fulfillments,
                assessor_fill,
            )
            .from(self.caller);
        tracing::trace!("Calldata: {}", call.calldata());
        let pending_tx = call.send().await?;
        tracing::debug!("Broadcasting tx {}", pending_tx.tx_hash());
        let tx_receipt = self.get_receipt_with_retry(pending_tx).await?;

        tracing::info!("Submitted merkle root and proof for batch {}", tx_receipt.transaction_hash);

        validate_fulfill_receipt(tx_receipt)
    }

    /// A combined call to `IBoundlessMarket.priceRequest` and `IBoundlessMarket.fulfill`.
    /// The caller should provide the signed request and signature for each unlocked request they
    /// want to fulfill. Payment for unlocked requests will go to the provided `prover` address.
    async fn price_and_fulfill(
        &self,
        unlocked_requests: Vec<UnlockedRequest>,
        fulfillments: Vec<Fulfillment>,
        assessor_fill: AssessorReceipt,
    ) -> Result<(), MarketError> {
        tracing::trace!("Calling priceAndFulfill({fulfillments:?}, {assessor_fill:?})");

        let (requests, client_sigs): (Vec<_>, Vec<_>) =
            unlocked_requests.into_iter().map(|ur| (ur.request, ur.client_sig)).unzip();
        let call = self
            .instance
            .priceAndFulfill(requests, client_sigs, fulfillments, assessor_fill)
            .from(self.caller);
        tracing::trace!("Calldata: {}", call.calldata());

        let pending_tx = call.send().await?;
        tracing::debug!("Broadcasting tx {}", pending_tx.tx_hash());

        let tx_receipt = self.get_receipt_with_retry(pending_tx).await?;

        tracing::info!("Fulfilled proof for batch {}", tx_receipt.transaction_hash);

        validate_fulfill_receipt(tx_receipt)
    }

    /// A combined call to `IBoundlessMarket.priceRequest` and `IBoundlessMarket.fulfillAndWithdraw`.
    /// The caller should provide the signed request and signature for each unlocked request they
    /// want to fulfill. Payment for unlocked requests will go to the provided `prover` address.
    async fn price_and_fulfill_and_withdraw(
        &self,
        unlocked_requests: Vec<UnlockedRequest>,
        fulfillments: Vec<Fulfillment>,
        assessor_fill: AssessorReceipt,
    ) -> Result<(), MarketError> {
        tracing::trace!("Calling priceAndFulfillAndWithdraw({fulfillments:?}, {assessor_fill:?})");

        let (requests, client_sigs): (Vec<_>, Vec<_>) =
            unlocked_requests.into_iter().map(|ur| (ur.request, ur.client_sig)).unzip();
        let call = self
            .instance
            .priceAndFulfillAndWithdraw(requests, client_sigs, fulfillments, assessor_fill)
            .from(self.caller);
        tracing::trace!("Calldata: {}", call.calldata());

        let pending_tx = call.send().await?;
        tracing::debug!("Broadcasting tx {}", pending_tx.tx_hash());

        let tx_receipt = self.get_receipt_with_retry(pending_tx).await?;

        tracing::info!("Fulfilled proof for batch {}", tx_receipt.transaction_hash);

        validate_fulfill_receipt(tx_receipt)
    }

    /// Combined function to submit a new merkle root to the set-verifier and call `priceAndfulfill`.
    /// Useful to reduce the transaction count for fulfillments
    async fn submit_root_and_price_fulfill(
        &self,
        root: Root,
        unlocked_requests: Vec<UnlockedRequest>,
        fulfillments: Vec<Fulfillment>,
        assessor_fill: AssessorReceipt,
    ) -> Result<(), MarketError> {
        let (requests, client_sigs): (Vec<_>, Vec<_>) =
            unlocked_requests.into_iter().map(|ur| (ur.request, ur.client_sig)).unzip();
        tracing::trace!("Calling submitRootAndPriceAndFulfill({:?}, {:x}, {:?}, {:?}, {fulfillments:?}, {assessor_fill:?})", root.root, root.seal, requests, client_sigs);
        let call = self
            .instance
            .submitRootAndPriceAndFulfill(
                root.verifier_address,
                root.root,
                root.seal,
                requests,
                client_sigs,
                fulfillments,
                assessor_fill,
            )
            .from(self.caller);
        tracing::trace!("Calldata: {}", call.calldata());
        let pending_tx = call.send().await?;
        tracing::debug!("Broadcasting tx {}", pending_tx.tx_hash());
        let tx_receipt = pending_tx
            .with_timeout(Some(self.timeout))
            .get_receipt()
            .await
            .context("failed to confirm tx")
            .map_err(MarketError::TxnConfirmationError)?;

        tracing::info!("Submitted merkle root and proof for batch {}", tx_receipt.transaction_hash);

        validate_fulfill_receipt(tx_receipt)
    }

    /// Combined function to submit a new merkle root to the set-verifier and call `priceAndFulfillAndWithdraw`.
    /// Useful to reduce the transaction count for fulfillments
    async fn submit_root_and_price_fulfill_and_withdraw(
        &self,
        root: Root,
        unlocked_requests: Vec<UnlockedRequest>,
        fulfillments: Vec<Fulfillment>,
        assessor_fill: AssessorReceipt,
    ) -> Result<(), MarketError> {
        let (requests, client_sigs): (Vec<_>, Vec<_>) =
            unlocked_requests.into_iter().map(|ur| (ur.request, ur.client_sig)).unzip();
        tracing::trace!("Calling submitRootAndPriceAndFulfillAndWithdraw({:?}, {:x}, {:?}, {:?}, {fulfillments:?}, {assessor_fill:?})", root.root, root.seal, requests, client_sigs);
        let call = self
            .instance
            .submitRootAndPriceAndFulfillAndWithdraw(
                root.verifier_address,
                root.root,
                root.seal,
                requests,
                client_sigs,
                fulfillments,
                assessor_fill,
            )
            .from(self.caller);
        tracing::trace!("Calldata: {}", call.calldata());
        let pending_tx = call.send().await?;
        tracing::debug!("Broadcasting tx {}", pending_tx.tx_hash());
        let tx_receipt = pending_tx
            .with_timeout(Some(self.timeout))
            .get_receipt()
            .await
            .context("failed to confirm tx")
            .map_err(MarketError::TxnConfirmationError)?;

        tracing::info!("Submitted merkle root and proof for batch {}", tx_receipt.transaction_hash);

        validate_fulfill_receipt(tx_receipt)
    }

    /// Checks if a request is locked in.
    pub async fn is_locked(&self, request_id: U256) -> Result<bool, MarketError> {
        tracing::trace!("Calling requestIsLocked({:x})", request_id);
        let res = self.instance.requestIsLocked(request_id).call().await?;

        Ok(res)
    }

    /// Checks if a request is fulfilled.
    pub async fn is_fulfilled(&self, request_id: U256) -> Result<bool, MarketError> {
        tracing::trace!("Calling requestIsFulfilled({:x})", request_id);
        let res = self.instance.requestIsFulfilled(request_id).call().await?;

        Ok(res)
    }

    /// Checks if a request is slashed.
    pub async fn is_slashed(&self, request_id: U256) -> Result<bool, MarketError> {
        tracing::trace!("Calling requestIsSlashed({:x})", request_id);
        let res = self.instance.requestIsSlashed(request_id).call().await?;

        Ok(res)
    }

    /// Returns the [RequestStatus] of a request.
    ///
    /// The `expires_at` parameter is the time at which the request expires.
    pub async fn get_status(
        &self,
        request_id: U256,
        expires_at: Option<u64>,
    ) -> Result<RequestStatus, MarketError> {
        let timestamp = self.get_latest_block_timestamp().await?;

        if self.is_fulfilled(request_id).await.context("Failed to check fulfillment status")? {
            return Ok(RequestStatus::Fulfilled);
        }

        if let Some(expires_at) = expires_at {
            if timestamp > expires_at {
                return Ok(RequestStatus::Expired);
            }
        }

        if self.is_locked(request_id).await.context("Failed to check locked status")? {
            let deadline = self.instance.requestDeadline(request_id).call().await?;
            if timestamp > deadline && deadline > 0 {
                return Ok(RequestStatus::Expired);
            };
            return Ok(RequestStatus::Locked);
        }

        Ok(RequestStatus::Unknown)
    }

    async fn get_latest_block_number(&self) -> Result<u64, MarketError> {
        Ok(self
            .instance
            .provider()
            .get_block_number()
            .await
            .context("Failed to get latest block number")?)
    }

    async fn get_latest_block_timestamp(&self) -> Result<u64, MarketError> {
        let block = self
            .instance
            .provider()
            .get_block_by_number(BlockNumberOrTag::Latest)
            .await
            .context("failed to get block")?
            .context("failed to get block")?;
        Ok(block.header.timestamp())
    }

    /// Retry a query operation with the configured retry settings.
    pub(crate) async fn retry_query<F, Fut, T>(
        &self,
        config: &EventQueryConfig,
        query_fn: F,
        function_name: &str,
    ) -> Result<T, MarketError>
    where
        F: Fn() -> Fut,
        Fut: std::future::Future<Output = Result<T, MarketError>>,
        MarketError: std::fmt::Debug,
    {
        let Some(retry_config) = &config.retry_config else {
            return query_fn().await;
        };

        let retries = retry_config.retries;
        if retries == 0 {
            return query_fn().await;
        }

        let mut last_error = None;
        for attempt in 0..=retries {
            match query_fn().await {
                Ok(result) => return Ok(result),
                Err(err) => {
                    if attempt < retries {
                        tracing::warn!(
                            "Operation [{}] failed: {err:?}, starting retry {}/{}",
                            function_name,
                            attempt + 1,
                            retries
                        );
                        if let Some(delay_ms) = retry_config.retry_delay_ms {
                            tokio::time::sleep(Duration::from_millis(delay_ms)).await;
                        }
                        last_error = Some(err);
                        continue;
                    }
                    last_error = Some(err);
                }
            }
        }

        tracing::warn!(
            "Operation [{}] failed after {} retries, returning last error: {:?}",
            function_name,
            retries,
            last_error
        );
        Err(last_error.unwrap())
    }

    /// Query the ProofDelivered event based on request ID and block options.
    /// For each iteration, we query a range of blocks.
    /// If the event is not found, we move the range down and repeat until we find the event.
    /// If the event is not found after the configured max iterations, we return an error.
    /// The default range is set to 1000 blocks for each iteration, and the default maximum number of
    /// iterations is 100. This means that the search will cover a maximum of 100,000 blocks.
    /// Optionally, you can specify a lower and upper bound to limit the search range.
    async fn query_fulfilled_event(
        &self,
        request_id: U256,
        lower_bound: Option<u64>,
        upper_bound: Option<u64>,
    ) -> Result<ProofDelivered, MarketError> {
        let config = self.event_query_config.clone();
        self.retry_query(&config, || async {
            let mut upper_block = upper_bound.unwrap_or(self.get_latest_block_number().await?);
            let initial_upper_block = upper_block;
            let start_block = lower_bound.unwrap_or(upper_block.saturating_sub(
                self.event_query_config.block_range * self.event_query_config.max_iterations,
            ));

            // Loop to progressively search through blocks
            tracing::debug!(
                "Querying ProofDelivered event for request ID {:x} in blocks {} to {} [iterations: {}]",
                request_id,
                start_block,
                upper_block,
                self.event_query_config.max_iterations
            );
            let mut final_block_checked = initial_upper_block;
            for _ in 0..self.event_query_config.max_iterations {
                // If the current end block is less than or equal to the starting block, stop searching
                if upper_block <= start_block {
                    break;
                }

                // Calculate the block range to query: from [lower_block] to [upper_block]
                let lower_block = upper_block.saturating_sub(self.event_query_config.block_range);

                // Set up the event filter for the specified block range
                let mut event_filter = self.instance.ProofDelivered_filter();
                tracing::trace!(
                    "Querying ProofDelivered event for request ID {:x} in blocks {} to {}",
                    request_id,
                    lower_block,
                    upper_block
                );
                event_filter.filter = event_filter
                    .filter
                    .topic1(request_id)
                    .from_block(lower_block)
                    .to_block(upper_block);

                // Query the logs for the event
                let logs = event_filter.query().await?;

                if let Some((event, _)) = logs.first() {
                    return Ok(event.clone());
                }

                // Track the last block we actually queried (lower_block is the start of the range we queried)
                final_block_checked = lower_block;

                // Move the upper_block down for the next iteration
                upper_block = lower_block.saturating_sub(1);
            }

            // Return error if no logs are found after all iterations
            Err(MarketError::ProofNotFound(request_id, initial_upper_block, final_block_checked))
        }, "query_fulfilled_event").await
    }

    /// Query the RequestSubmitted event based on request ID and block options.
    ///
    /// For each iteration, we query a range of blocks.
    /// If the event is not found, we move the range down and repeat until we find the event.
    /// If the event is not found after the configured max iterations, we return an error.
    /// The default range is set to 1000 blocks for each iteration, and the default maximum number of
    /// iterations is 100. This means that the search will cover a maximum of 100,000 blocks.
    /// Optionally, you can specify a lower and upper bound to limit the search range.
    pub async fn query_request_submitted_event(
        &self,
        request_id: U256,
        lower_bound: Option<u64>,
        upper_bound: Option<u64>,
    ) -> Result<RequestSubmittedEventData, MarketError> {
        let config = self.event_query_config.clone();
        self.retry_query(&config, || async {
            let mut upper_block = upper_bound.unwrap_or(self.get_latest_block_number().await?);
            let start_block = lower_bound.unwrap_or(upper_block.saturating_sub(
                self.event_query_config.block_range * self.event_query_config.max_iterations,
            ));

            let iterations = if lower_bound.is_some() && upper_bound.is_some() {
                ((upper_block - start_block) / self.event_query_config.block_range).saturating_add(1)
            } else {
                self.event_query_config.max_iterations
            };

            // Loop to progressively search through blocks
            tracing::debug!("Querying RequestSubmitted event for request ID {:x} in blocks {} to {} [iterations: {}]", request_id, start_block, upper_block, iterations);
            for _ in 0..iterations {
                // If the current end block is less than or equal to the starting block, stop searching
                if upper_block <= start_block {
                    break;
                }

                // Calculate the block range to query: from [lower_block] to [upper_block]
                let lower_block = upper_block.saturating_sub(self.event_query_config.block_range);

                // Set up the event filter for the specified block range
                let mut event_filter = self.instance.RequestSubmitted_filter();
                tracing::trace!(
                    "Querying RequestSubmitted event for request ID {:x} in blocks {} to {}",
                    request_id,
                    lower_block,
                    upper_block
                );
                event_filter.filter = event_filter
                    .filter
                    .topic1(request_id)
                    .from_block(lower_block)
                    .to_block(upper_block);

                // Query the logs for the event
                let logs = event_filter.query().await?;

                if let Some((event, log_meta)) = logs.first() {
                    let block_num = log_meta.block_number.unwrap_or(0);
                    let tx_hash = log_meta.transaction_hash.unwrap_or(B256::ZERO);
                    return Ok(RequestSubmittedEventData {
                        request: event.request.clone(),
                        client_signature: event.clientSignature.clone(),
                        block_number: block_num,
                        tx_hash,
                    });
                }

                // Move the upper_block down for the next iteration
                upper_block = lower_block.saturating_sub(1);
            }

            // Return error if no logs are found after all iterations
            Err(MarketError::RequestNotFound(request_id))
        }, "query_request_submitted_event").await
    }

    /// Query the RequestLocked event based on request ID and block options.
    ///
    /// Returns the event data and the block number where it occurred.
    /// For each iteration, we query a range of blocks.
    /// If the event is not found, we move the range down and repeat until we find the event.
    /// If the event is not found after the configured max iterations, we return an error.
    /// The default range is set to 1000 blocks for each iteration, and the default maximum number of
    /// iterations is 100. This means that the search will cover a maximum of 100,000 blocks.
    /// Optionally, you can specify a lower and upper bound to limit the search range.
    /// Specifying both a lower and upper bound will override configured max iterations, ensuring your search covers the specified range.
    pub async fn query_request_locked_event(
        &self,
        request_id: U256,
        lower_bound: Option<u64>,
        upper_bound: Option<u64>,
    ) -> Result<RequestLockedEventData, MarketError> {
        let config = self.event_query_config.clone();
        self.retry_query(&config, || async {
            let mut upper_block = upper_bound.unwrap_or(self.get_latest_block_number().await?);
            let start_block = lower_bound.unwrap_or(upper_block.saturating_sub(
                self.event_query_config.block_range * self.event_query_config.max_iterations,
            ));

            let iterations = if lower_bound.is_some() && upper_bound.is_some() {
                ((upper_block - start_block) / self.event_query_config.block_range).saturating_add(1)
            } else {
                self.event_query_config.max_iterations
            };

            tracing::debug!(
                "Querying RequestLocked event for request ID {:x} in blocks {} to {} [iterations: {}]",
                request_id,
                start_block,
                upper_block,
                iterations
            );
            // Loop to progressively search through blocks
            for _ in 0..iterations {
                // If the current end block is less than or equal to the starting block, stop searching
                if upper_block <= start_block {
                    break;
                }

                // Calculate the block range to query: from [lower_block] to [upper_block]
                let lower_block = upper_block.saturating_sub(self.event_query_config.block_range);

                // Set up the event filter for the specified block range
                let mut event_filter = self.instance.RequestLocked_filter();
                tracing::trace!(
                    "Querying RequestLocked event for request ID {:x} in blocks {} to {}",
                    request_id,
                    lower_block,
                    upper_block
                );
                event_filter.filter = event_filter
                    .filter
                    .topic1(request_id)
                    .from_block(lower_block)
                    .to_block(upper_block);

                // Query the logs for the event
                let logs = event_filter.query().await?;

                if let Some((event, log_meta)) = logs.first() {
                    let block_num = log_meta.block_number.unwrap_or(0);
                    let tx_hash = log_meta.transaction_hash.unwrap_or(B256::ZERO);
                    return Ok(RequestLockedEventData {
                        event: event.clone(),
                        block_number: block_num,
                        tx_hash,
                    });
                }

                // Move the upper_block down for the next iteration
                upper_block = lower_block.saturating_sub(1);
            }

            // Return error if no logs are found after all iterations
            Err(MarketError::RequestNotFound(request_id))
        }, "query_request_locked_event").await
    }

    /// Query the RequestFulfilled event based on request ID and block options.
    ///
    /// Returns the event data and the block number where it occurred.
    /// This event is emitted once when a request is first fulfilled.
    /// Specifying both a lower and upper bound will override configured max iterations, ensuring your search covers the specified range.
    pub async fn query_request_fulfilled_event(
        &self,
        request_id: U256,
        lower_bound: Option<u64>,
        upper_bound: Option<u64>,
    ) -> Result<RequestFulfilledEventData, MarketError> {
        let config = self.event_query_config.clone();
        self.retry_query(&config, || async {
            let mut upper_block = upper_bound.unwrap_or(self.get_latest_block_number().await?);
            let start_block = lower_bound.unwrap_or(upper_block.saturating_sub(
                self.event_query_config.block_range * self.event_query_config.max_iterations,
            ));

            let iterations = if lower_bound.is_some() && upper_bound.is_some() {
                ((upper_block - start_block) / self.event_query_config.block_range).saturating_add(1)
            } else {
                self.event_query_config.max_iterations
            };

            tracing::debug!("Querying RequestFulfilled event for request ID {:x} in blocks {} to {} [iterations: {}]", request_id, start_block, upper_block, iterations);
            // Loop to progressively search through blocks
            for _ in 0..iterations {
                // If the current end block is less than or equal to the starting block, stop searching
                if upper_block <= start_block {
                    break;
                }

                // Calculate the block range to query: from [lower_block] to [upper_block]
                let lower_block = upper_block.saturating_sub(self.event_query_config.block_range);

                // Set up the event filter for the specified block range
                let mut event_filter = self.instance.RequestFulfilled_filter();
                tracing::trace!(
                    "Querying RequestFulfilled event for request ID {:x} in blocks {} to {}",
                    request_id,
                    lower_block,
                    upper_block
                );
                event_filter.filter = event_filter
                    .filter
                    .topic1(request_id)
                    .from_block(lower_block)
                    .to_block(upper_block);

                // Query the logs for the event
                let logs = event_filter.query().await?;

                if let Some((event, log_meta)) = logs.first() {
                    let block_num = log_meta.block_number.unwrap_or(0);
                    let tx_hash = log_meta.transaction_hash.unwrap_or(B256::ZERO);
                    return Ok(RequestFulfilledEventData {
                        event: event.clone(),
                        block_number: block_num,
                        tx_hash,
                    });
                }

                // Move the upper_block down for the next iteration
                upper_block = lower_block.saturating_sub(1);
            }

            // Return error if no logs are found after all iterations
            Err(MarketError::RequestNotFound(request_id))
        }, "query_request_fulfilled_event").await
    }

    /// Query ALL ProofDelivered events for a request ID across block range.
    ///
    /// Returns a vector of all ProofDelivered events with their block numbers.
    /// This is useful because multiple proofs can be delivered for a single request until timeout.
    /// Specifying both a lower and upper bound will override configured max iterations, ensuring your search covers the specified range.
    pub async fn query_all_proof_delivered_events(
        &self,
        request_id: U256,
        lower_bound: Option<u64>,
        upper_bound: Option<u64>,
    ) -> Result<Vec<ProofDeliveredEventData>, MarketError> {
        let config = self.event_query_config.clone();
        self.retry_query(&config, || async {
            let mut all_events = Vec::new();
            let mut upper_block = upper_bound.unwrap_or(self.get_latest_block_number().await?);
            let start_block = lower_bound.unwrap_or(upper_block.saturating_sub(
                self.event_query_config.block_range * self.event_query_config.max_iterations,
            ));

            let iterations = if lower_bound.is_some() && upper_bound.is_some() {
                ((upper_block - start_block) / self.event_query_config.block_range).saturating_add(1)
            } else {
                self.event_query_config.max_iterations
            };

            // Loop to progressively search through blocks
            tracing::debug!("Querying all ProofDelivered events for request ID {:x} in blocks {} to {} [iterations: {}]", request_id, start_block, upper_block, iterations);
            for _ in 0..iterations {
                // If the current end block is less than or equal to the starting block, stop searching
                if upper_block <= start_block {
                    break;
                }

                // Calculate the block range to query: from [lower_block] to [upper_block]
                let lower_block = upper_block.saturating_sub(self.event_query_config.block_range);

                // Set up the event filter for the specified block range
                let mut event_filter = self.instance.ProofDelivered_filter();
                tracing::trace!(
                    "Querying ProofDelivered event for request ID {:x} in blocks {} to {}",
                    request_id,
                    lower_block,
                    upper_block
                );
                event_filter.filter = event_filter
                    .filter
                    .topic1(request_id)
                    .from_block(lower_block)
                    .to_block(upper_block);

                // Query the logs for the event
                let logs = event_filter.query().await?;

                // Collect all events with block numbers from this range
                for (event, log_meta) in logs {
                    let block_num = log_meta.block_number.unwrap_or(0);
                    let tx_hash = log_meta.transaction_hash.unwrap_or(B256::ZERO);
                    all_events.push(ProofDeliveredEventData {
                        event,
                        block_number: block_num,
                        tx_hash,
                    });
                }

                // Move the upper_block down for the next iteration
                upper_block = lower_block.saturating_sub(1);
            }

            Ok(all_events)
        }, "query_all_proof_delivered_events").await
    }

    /// Query ProverSlashed event for a request ID.
    ///
    /// Returns the slash event with its block number if found.
    pub async fn query_prover_slashed_event(
        &self,
        request_id: U256,
        lower_bound: Option<u64>,
        upper_bound: Option<u64>,
    ) -> Result<ProverSlashedEventData, MarketError> {
        let config = self.event_query_config.clone();
        self.retry_query(
            &config,
            || async {
                let mut upper_block = upper_bound.unwrap_or(self.get_latest_block_number().await?);
                let start_block = lower_bound.unwrap_or(upper_block.saturating_sub(
                    self.event_query_config.block_range * self.event_query_config.max_iterations,
                ));

                let iterations = if lower_bound.is_some() && upper_bound.is_some() {
                    ((upper_block - start_block) / self.event_query_config.block_range)
                        .saturating_add(1)
                } else {
                    self.event_query_config.max_iterations
                };

                // Loop to progressively search through blocks
                for _ in 0..iterations {
                    // If the current end block is less than or equal to the starting block, stop searching
                    if upper_block <= start_block {
                        break;
                    }

                    // Calculate the block range to query: from [lower_block] to [upper_block]
                    let lower_block =
                        upper_block.saturating_sub(self.event_query_config.block_range);

                    // Set up the event filter for the specified block range
                    let mut event_filter = self.instance.ProverSlashed_filter();
                    event_filter.filter = event_filter
                        .filter
                        .topic1(request_id)
                        .from_block(lower_block)
                        .to_block(upper_block);

                    // Query the logs for the event
                    let logs = event_filter.query().await?;

                    if let Some((event, log_meta)) = logs.first() {
                        let block_num = log_meta.block_number.unwrap_or(0);
                        let tx_hash = log_meta.transaction_hash.unwrap_or(B256::ZERO);
                        return Ok(ProverSlashedEventData {
                            event: event.clone(),
                            block_number: block_num,
                            tx_hash,
                        });
                    }

                    // Move the upper_block down for the next iteration
                    upper_block = lower_block.saturating_sub(1);
                }

                // Return error if no logs are found after all iterations
                Err(MarketError::RequestNotFound(request_id))
            },
            "query_prover_slashed_event",
        )
        .await
    }

    /// Returns fulfillment data and seal if the request is fulfilled.
    pub async fn get_request_fulfillment(
        &self,
        request_id: U256,
    ) -> Result<Fulfillment, MarketError> {
        match self.get_status(request_id, None).await? {
            RequestStatus::Expired => Err(MarketError::RequestHasExpired(request_id)),
            RequestStatus::Fulfilled => {
                let event = self.query_fulfilled_event(request_id, None, None).await?;
                Ok(event.fulfillment)
            }
            _ => Err(MarketError::RequestNotFulfilled(request_id)),
        }
    }

    /// Returns the prover address for a request that is fulfilled.
    pub async fn get_request_fulfillment_prover(
        &self,
        request_id: U256,
    ) -> Result<Address, MarketError> {
        match self.get_status(request_id, None).await? {
            RequestStatus::Expired => Err(MarketError::RequestHasExpired(request_id)),
            RequestStatus::Fulfilled => {
                let event = self.query_fulfilled_event(request_id, None, None).await?;
                Ok(event.prover)
            }
            _ => Err(MarketError::RequestNotFulfilled(request_id)),
        }
    }

    /// Returns proof request and signature for a request submitted onchain.
    pub async fn get_submitted_request(
        &self,
        request_id: U256,
        tx_hash: Option<B256>,
    ) -> Result<(ProofRequest, Bytes), MarketError> {
        if let Some(tx_hash) = tx_hash {
            let tx_data = self
                .instance
                .provider()
                .get_transaction_by_hash(tx_hash)
                .await
                .context("Failed to get transaction")?
                .context("Transaction not found")?;
            let inputs = tx_data.input();
            // TODO: This should parse from events rather than calldata.
            let calldata = IBoundlessMarket::submitRequestCall::abi_decode(inputs)
                .context("Failed to decode input")?;
            return Ok((calldata.request, calldata.clientSignature));
        }
        let data = self.query_request_submitted_event(request_id, None, None).await?;
        Ok((data.request, data.client_signature))
    }

    /// Returns the fulfillment data and seal if the request is fulfilled.
    ///
    /// This method will poll the status of the request until it is Fulfilled or Expired.
    /// Polling is done at intervals of `retry_interval` until the request is Fulfilled, Expired or
    /// the optional timeout is reached.
    pub async fn wait_for_request_fulfillment(
        &self,
        request_id: U256,
        retry_interval: Duration,
        expires_at: u64,
    ) -> Result<Fulfillment, MarketError> {
        loop {
            let status = self.get_status(request_id, Some(expires_at)).await?;
            match status {
                RequestStatus::Expired => return Err(MarketError::RequestHasExpired(request_id)),
                RequestStatus::Fulfilled => {
                    let event = self.query_fulfilled_event(request_id, None, None).await?;
                    return Ok(event.fulfillment);
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
        let request_id = RequestId::u256(self.caller, id);
        match self.get_status(request_id, None).await? {
            RequestStatus::Unknown => Ok(id),
            _ => Err(MarketError::Error(anyhow!("index already in use"))),
        }
    }

    /// Generates a new request ID based on the EOA nonce.
    ///
    /// It does not guarantee that the ID is not in use by the time the caller uses it.
    pub async fn request_id_from_nonce(&self) -> Result<U256, MarketError> {
        let index = self.index_from_nonce().await?;
        Ok(RequestId::u256(self.caller, index))
    }

    /// Randomly generates a request index.
    ///
    /// It retries up to 10 times to generate a unique index, after which it returns an error.
    /// It does not guarantee that the index is not in use by the time the caller uses it.
    pub async fn index_from_rand(&self) -> Result<u32, MarketError> {
        let attempts = 10usize;
        for _ in 0..attempts {
            let id: u32 = rand::random();
            let request_id = RequestId::u256(self.caller, id);
            match self.get_status(request_id, None).await? {
                RequestStatus::Unknown => return Ok(id),
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
        Ok(RequestId::u256(self.caller, index))
    }

    /// Returns the image ID and URL of the assessor guest.
    pub async fn image_info(&self) -> Result<(B256, String)> {
        tracing::trace!("Calling imageInfo()");
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

    /// Approves Boundless Market contract to deposit `value` amount of collateral on behalf of the caller.
    pub async fn approve_deposit_collateral(&self, value: U256) -> Result<()> {
        let spender = *self.instance.address();
        tracing::trace!("Calling approve({:?}, {})", spender, value);
        let token_address = self
            .instance
            .COLLATERAL_TOKEN_CONTRACT()
            .call()
            .await
            .context("COLLATERAL_TOKEN_CONTRACT call failed")?
            .0;
        let contract = IERC20::new(token_address.into(), self.instance.provider());
        let call = contract.approve(spender, value).from(self.caller);
        let pending_tx = call.send().await.map_err(IHitPointsErrors::decode_error)?;
        tracing::debug!("Broadcasting tx {}", pending_tx.tx_hash());
        let tx_hash = pending_tx
            .with_timeout(Some(self.timeout))
            .watch()
            .await
            .context("failed to confirm tx")?;

        tracing::debug!(
            "Approved {} to spend {} of token 0x{:x}. Tx hash: {}",
            spender,
            value,
            token_address,
            tx_hash
        );

        Ok(())
    }

    /// Deposit collateral into the market to pay for lock operations.
    ///
    /// Before calling this method, the account owner must first approve
    /// the Boundless market contract as an allowed spender by calling `approve_deposit_collateral`.    
    pub async fn deposit_collateral(&self, value: U256) -> Result<(), MarketError> {
        tracing::trace!("Calling depositCollateral({})", value);
        let call = self.instance.depositCollateral(value);
        tracing::debug!("Sending tx {}", format!("{:?}", call));
        tracing::debug!("Market address: {:?}", self.instance.address());
        let pending_tx = call.send().await?;
        tracing::debug!(
            "Broadcasting {} collateral deposit to market {:?}. Tx hash: {}",
            value,
            self.instance.address(),
            pending_tx.tx_hash()
        );
        let tx_hash = pending_tx
            .with_timeout(Some(self.timeout))
            .watch()
            .await
            .context("failed to confirm tx")?;
        tracing::debug!(
            "Submitted {} collateral deposit to market {:?}. Tx hash: {}",
            value,
            self.instance.address(),
            tx_hash
        );
        Ok(())
    }

    /// Permit and deposit collateral into the market to pay for lockin collateral.
    ///
    /// WARNING: The collateral tokens on some networks do not support permit. To ensure successful deposits regardless of the network, use `approve_deposit_collateral` and `deposit_collateral` instead.
    pub async fn deposit_collateral_with_permit(
        &self,
        value: U256,
        signer: &impl Signer,
    ) -> Result<(), MarketError> {
        if !collateral_token_supports_permit(self.get_chain_id().await?) {
            return Err(MarketError::Error(anyhow!("Collateral token does not support permit. Use approve_deposit_collateral and deposit_collateral instead.")));
        }
        let token_address = self
            .instance
            .COLLATERAL_TOKEN_CONTRACT()
            .call()
            .await
            .context("COLLATERAL_TOKEN_CONTRACT call failed")?
            .0;
        let contract = IERC20Permit::new(token_address.into(), self.instance.provider());
        let call = contract.nonces(self.caller());
        let nonce = call.call().await.map_err(IHitPointsErrors::decode_error)?;
        let block = self
            .instance
            .provider()
            .get_block_by_number(BlockNumberOrTag::Latest)
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
        let domain_separator = contract.DOMAIN_SEPARATOR().call().await?;
        let sig = permit.sign(signer, domain_separator).await?.as_bytes();
        let r = B256::from_slice(&sig[..32]);
        let s = B256::from_slice(&sig[32..64]);
        let v: u8 = sig[64];
        tracing::trace!("Calling depositStakeWithPermit({})", value);
        let call = self.instance.depositCollateralWithPermit(value, deadline, v, r, s);
        let pending_tx = call.send().await?;
        tracing::debug!(
            "Broadcasting {} collateral deposit to market {:?}. Tx hash: {}",
            value,
            self.instance.address(),
            pending_tx.tx_hash()
        );
        let tx_hash = pending_tx
            .with_timeout(Some(self.timeout))
            .watch()
            .await
            .context("failed to confirm tx")?;
        tracing::debug!(
            "Submitted {} collateral deposit to market {:?}. Tx hash: {}",
            value,
            self.instance.address(),
            tx_hash
        );
        Ok(())
    }

    /// Withdraw collateral from the market.
    pub async fn withdraw_collateral(&self, value: U256) -> Result<(), MarketError> {
        tracing::trace!("Calling withdrawStake({})", value);
        let call = self.instance.withdrawCollateral(value);
        let pending_tx = call.send().await?;
        tracing::debug!(
            "Broadcasting {} collateral withdraw to market {:?}. Tx hash: {}",
            value,
            self.instance.address(),
            pending_tx.tx_hash()
        );
        let tx_hash = pending_tx
            .with_timeout(Some(self.timeout))
            .watch()
            .await
            .context("failed to confirm tx")?;
        tracing::debug!(
            "Submitted {} collateral withdraw to market {:?}. Tx hash: {}",
            value,
            self.instance.address(),
            tx_hash
        );
        self.check_collateral_balance().await?;
        Ok(())
    }

    /// Returns the deposited balance, in HP, of the given account.
    pub async fn balance_of_collateral(
        &self,
        account: impl Into<Address>,
    ) -> Result<U256, MarketError> {
        let account = account.into();
        tracing::trace!("Calling balanceOfCollateral({})", account);
        let balance =
            self.instance.balanceOfCollateral(account).call().await.context("call failed")?;
        Ok(balance)
    }

    /// Check the current collateral balance against the alert config
    /// and log a warning or error or below the thresholds.
    async fn check_collateral_balance(&self) -> Result<(), MarketError> {
        let collateral_balance = self.balance_of_collateral(self.caller()).await?;
        if collateral_balance < self.balance_alert_config.error_threshold.unwrap_or(U256::ZERO) {
            tracing::error!(
                "[B-BAL-STK] collateral balance {} for {} < error threshold",
                collateral_balance,
                self.caller(),
            );
        } else if collateral_balance
            < self.balance_alert_config.warn_threshold.unwrap_or(U256::ZERO)
        {
            tracing::warn!(
                "[B-BAL-STK] collateral balance {} for {} < warning threshold",
                collateral_balance,
                self.caller(),
            );
        } else {
            tracing::trace!("collateral balance for {} is: {}", self.caller(), collateral_balance);
        }
        Ok(())
    }

    /// Returns the collateral token address used by the market.
    pub async fn collateral_token_address(&self) -> Result<Address, MarketError> {
        tracing::trace!("Calling COLLATERAL_TOKEN_CONTRACT()");
        let address = self
            .instance
            .COLLATERAL_TOKEN_CONTRACT()
            .call()
            .await
            .context("COLLATERAL_TOKEN_CONTRACT call failed")?
            .0;
        Ok(address.into())
    }

    /// Returns the collateral token's symbol.
    pub async fn collateral_token_symbol(&self) -> Result<String, MarketError> {
        let address = self.collateral_token_address().await?;
        let contract = IERC20::new(address, self.instance.provider());
        let symbol = contract.symbol().call().await.context("Failed to get token symbol")?;
        Ok(symbol)
    }

    /// Returns the collateral token's decimals.
    pub async fn collateral_token_decimals(&self) -> Result<u8, MarketError> {
        let address = self.collateral_token_address().await?;
        let contract = IERC20::new(address, self.instance.provider());
        let decimals = contract.decimals().call().await.context("Failed to get token decimals")?;
        Ok(decimals)
    }
}

impl Offer {
    /// Calculates the time, in seconds since the UNIX epoch, at which the price will be at the given price.
    pub fn time_at_price(&self, price: U256) -> Result<u64, MarketError> {
        let max_price = U256::from(self.maxPrice);
        let min_price = U256::from(self.minPrice);

        if price > U256::from(max_price) {
            return Err(MarketError::Error(anyhow::anyhow!("Price cannot exceed max price")));
        }

        if price <= min_price {
            return Ok(0);
        }

        let rise = max_price - min_price;
        let run = U256::from(self.rampUpPeriod);
        let delta = ((price - min_price) * run).div_ceil(rise);
        let delta: u64 = delta.try_into().context("Failed to convert block delta to u64")?;

        Ok(self.rampUpStart + delta)
    }

    /// Calculates the price at the given time, in seconds since the UNIX epoch.
    pub fn price_at(&self, timestamp: u64) -> Result<U256, MarketError> {
        Ok(crate::contracts::pricing::price_at_time(
            U256::from(self.minPrice),
            U256::from(self.maxPrice),
            self.rampUpStart,
            self.rampUpPeriod,
            self.lockTimeout,
            timestamp,
        ))
    }

    /// UNIX timestamp after which the request is considered completely expired.
    pub fn deadline(&self) -> u64 {
        self.rampUpStart + (self.timeout as u64)
    }

    /// UNIX timestamp after which any lock on the request expires, and the client fee is zero.
    ///
    /// Once locked, if a valid proof is not submitted before this deadline, the prover can be
    /// "slashed", which refunds the price to the requester and takes the prover collateral.
    /// Additionally, the fee paid by the client is zero for proofs delivered after this time. Note
    /// that after this time, and before `timeout` a proof can still be delivered to fulfill the
    /// request.
    pub fn lock_deadline(&self) -> u64 {
        self.rampUpStart + (self.lockTimeout as u64)
    }

    /// Returns the amount of collateral that the protocol awards to the prover who fills an order that
    /// was locked by another prover but not fulfilled by lock expiry.
    pub fn collateral_reward_if_locked_and_not_fulfilled(&self) -> U256 {
        self.lockCollateral
            .checked_mul(U256::from(FRACTION_STAKE_NUMERATOR))
            .unwrap()
            .checked_div(U256::from(FRACTION_STAKE_DENOMINATOR))
            .unwrap()
    }
}

#[derive(Debug, Clone)]
/// Represents the parameters for submitting a Merkle Root.
pub struct Root {
    /// The address of the set verifier contract.
    pub verifier_address: Address,
    /// The Merkle root of the proof.
    pub root: B256,
    /// The seal of the proof.
    pub seal: Bytes,
}

#[derive(Debug, Clone)]
/// Represents the parameters for pricing an unlocked request.
pub struct UnlockedRequest {
    /// The unlocked request to be priced.
    pub request: ProofRequest,
    /// The client signature for the request.
    pub client_sig: Bytes,
}

impl UnlockedRequest {
    /// Creates a new instance of the `UnlockedRequest` struct.
    pub fn new(request: ProofRequest, client_sig: impl Into<Bytes>) -> Self {
        Self { request, client_sig: client_sig.into() }
    }
}

#[derive(Clone)]
#[non_exhaustive]
/// Struct for creating a fulfillment transaction request.
///
/// The `root` can be `None` if the caller does not want to submit a new Merkle root as part of the transaction.
/// The `unlocked_requests` field is used to price the requests.
/// The `withdraw` field indicates whether the prover should withdraw their balance after fulfilling the requests.
pub struct FulfillmentTx {
    /// The parameters for submitting a Merkle Root
    pub root: Option<Root>,
    /// The list of unlocked requests.
    pub unlocked_requests: Vec<UnlockedRequest>,
    /// The fulfillments to be submitted
    pub fulfillments: Vec<Fulfillment>,
    /// The assessor receipt
    pub assessor_receipt: AssessorReceipt,
    /// Whether to withdraw the fee
    pub withdraw: bool,
}

impl FulfillmentTx {
    /// Creates a new instance of the `Fulfill` struct.
    pub fn new(fulfillments: Vec<Fulfillment>, assessor_receipt: AssessorReceipt) -> Self {
        Self {
            root: None,
            unlocked_requests: Vec::new(),
            fulfillments,
            assessor_receipt,
            withdraw: false,
        }
    }

    /// Sets the parameters for submitting a Merkle Root.
    pub fn with_submit_root(
        self,
        verifier_address: impl Into<Address>,
        root: B256,
        seal: impl Into<Bytes>,
    ) -> Self {
        Self {
            root: Some(Root { verifier_address: verifier_address.into(), root, seal: seal.into() }),
            ..self
        }
    }

    /// Adds an unlocked request to be priced to the transaction.
    pub fn with_unlocked_request(self, unlocked_request: UnlockedRequest) -> Self {
        let mut requests = self.unlocked_requests;
        requests.push(unlocked_request);
        Self { unlocked_requests: requests, ..self }
    }

    /// Adds a list of unlocked requests to be priced to the transaction.
    pub fn with_unlocked_requests(self, unlocked_requests: Vec<UnlockedRequest>) -> Self {
        let mut requests = self.unlocked_requests;
        requests.extend(unlocked_requests);
        Self { unlocked_requests: requests, ..self }
    }

    /// Sets whether to withdraw the fee.
    pub fn with_withdraw(self, withdraw: bool) -> Self {
        Self { withdraw, ..self }
    }
}

#[cfg(test)]
mod tests {
    use crate::contracts::Offer;
    use alloy::primitives::{utils::parse_ether, U256};
    use tracing_test::traced_test;
    fn ether(value: &str) -> U256 {
        parse_ether(value).unwrap()
    }

    fn test_offer(bidding_start: u64) -> Offer {
        Offer {
            minPrice: ether("1"),
            maxPrice: ether("2"),
            rampUpStart: bidding_start,
            rampUpPeriod: 100,
            timeout: 500,
            lockTimeout: 500,
            lockCollateral: ether("1"),
        }
    }

    #[test]
    fn test_price_at() {
        let offer = &test_offer(100);

        // Before bidding start, price is min price.
        assert_eq!(offer.price_at(90).unwrap(), ether("1"));

        assert_eq!(offer.price_at(100).unwrap(), ether("1"));

        assert_eq!(offer.price_at(101).unwrap(), ether("1.01"));
        assert_eq!(offer.price_at(125).unwrap(), ether("1.25"));
        assert_eq!(offer.price_at(150).unwrap(), ether("1.5"));
        assert_eq!(offer.price_at(175).unwrap(), ether("1.75"));
        assert_eq!(offer.price_at(199).unwrap(), ether("1.99"));

        assert_eq!(offer.price_at(200).unwrap(), ether("2"));
        assert_eq!(offer.price_at(500).unwrap(), ether("2"));
    }

    #[test]
    fn test_time_at_price() {
        let offer = &test_offer(100);

        assert_eq!(offer.time_at_price(ether("1")).unwrap(), 0);

        assert_eq!(offer.time_at_price(ether("1.01")).unwrap(), 101);
        assert_eq!(offer.time_at_price(ether("1.001")).unwrap(), 101);

        assert_eq!(offer.time_at_price(ether("1.25")).unwrap(), 125);
        assert_eq!(offer.time_at_price(ether("1.5")).unwrap(), 150);
        assert_eq!(offer.time_at_price(ether("1.75")).unwrap(), 175);
        assert_eq!(offer.time_at_price(ether("1.99")).unwrap(), 199);
        assert_eq!(offer.time_at_price(ether("2")).unwrap(), 200);

        // Price cannot exceed maxPrice
        assert!(offer.time_at_price(ether("3")).is_err());
    }

    #[test]
    fn test_collateral_reward_if_locked_and_not_fulfilled() {
        let offer = &test_offer(100);
        assert_eq!(offer.collateral_reward_if_locked_and_not_fulfilled(), ether("0.5"));
    }

    #[tokio::test]
    #[traced_test]
    async fn test_retry_query_success_after_retry() {
        use super::*;
        use std::sync::{
            atomic::{AtomicU32, Ordering},
            Arc,
        };

        let config = EventQueryConfig {
            block_range: 1000,
            max_iterations: 100,
            retry_config: Some(EventRetryConfig { retries: 2, retry_delay_ms: None }), // No delay for test
        };

        let counter = Arc::new(AtomicU32::new(0));
        let counter_clone = counter.clone();

        let anvil = alloy::node_bindings::Anvil::new().spawn();
        let provider = alloy::providers::ProviderBuilder::new().connect_http(anvil.endpoint_url());
        let service = BoundlessMarketService::new(Address::ZERO, provider, Address::ZERO);

        let result = service
            .retry_query(
                &config,
                || {
                    let counter = counter_clone.clone();
                    async move {
                        let current = counter.fetch_add(1, Ordering::SeqCst);
                        if current == 0 || current == 1 {
                            Err(MarketError::Error(anyhow::anyhow!("Attempt {} failed", current)))
                        } else {
                            Ok(current)
                        }
                    }
                },
                "test_operation",
            )
            .await;

        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 2);
        assert_eq!(counter.load(Ordering::SeqCst), 3); // Initial + 2 retries

        // Verify retry logs were emitted
        assert!(logs_contain("Operation [test_operation] failed"));
        assert!(logs_contain("starting retry 1/2"));
        assert!(logs_contain("starting retry 2/2"));
    }

    #[tokio::test]
    #[traced_test]
    async fn test_query_fulfilled_event_retries() {
        use super::*;
        use alloy::node_bindings::Anvil;
        use boundless_test_utils::market::create_test_ctx;

        let anvil = Anvil::new().spawn();
        let ctx = create_test_ctx(&anvil).await.unwrap();

        // Create a service with retry config
        let mut market_service = BoundlessMarketService::new(
            ctx.deployment.boundless_market_address,
            ctx.prover_provider.clone(),
            ctx.prover_signer.address(),
        );
        market_service = market_service.with_event_query_config(EventQueryConfig {
            block_range: 1000,
            max_iterations: 10,
            retry_config: Some(EventRetryConfig { retries: 1, retry_delay_ms: None }), // No delay for test
        });

        // Query for a non-existent request ID - this should trigger retries
        let non_existent_id = U256::from(999999);
        let result = market_service.query_fulfilled_event(non_existent_id, None, None).await;

        // Should fail after retries
        assert!(result.is_err());

        // Verify retry logs were emitted
        assert!(logs_contain("Operation [query_fulfilled_event] failed"));
        assert!(logs_contain("starting retry 1/1"));
        assert!(logs_contain("failed after 1 retries"));
    }
}
