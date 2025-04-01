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
    consensus::{BlockHeader, Transaction},
    eips::BlockNumberOrTag,
    network::Ethereum,
    primitives::{Address, Bytes, B256, U256},
    providers::Provider,
    rpc::types::{Log, TransactionReceipt},
    signers::Signer,
};
use alloy_sol_types::{SolCall, SolEvent};
use anyhow::{anyhow, Context, Result};
use risc0_ethereum_contracts::event_query::EventQueryConfig;
use thiserror::Error;

use crate::contracts::token::{IERC20Permit, IHitPoints::IHitPointsErrors, Permit, IERC20};

use super::{
    eip712_domain, AssessorReceipt, EIP712DomainSaltless, Fulfillment,
    IBoundlessMarket::{self, IBoundlessMarketInstance},
    Offer, ProofRequest, RequestError, RequestId, RequestStatus, TxnErr, TXN_CONFIRM_TIMEOUT,
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

    /// Lock request reverted, possibly outbid.
    #[error("Lock request reverted, possibly outbid: txn_hash: {0}")]
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
        tracing::debug!("raw alloy contract error: {:?}", err);
        MarketError::Error(TxnErr::from(err).into())
    }
}

/// Proof market service.
pub struct BoundlessMarketService<P> {
    instance: IBoundlessMarketInstance<(), P, Ethereum>,
    // Chain ID with caching to ensure we fetch it at most once.
    chain_id: AtomicU64,
    caller: Address,
    timeout: Duration,
    event_query_config: EventQueryConfig,
    balance_alert_config: StakeBalanceAlertConfig,
}

#[derive(Clone, Debug, Default)]
struct StakeBalanceAlertConfig {
    /// Threshold at which to log a warning
    warn_threshold: Option<U256>,
    /// Threshold at which to log an error
    error_threshold: Option<U256>,
}

impl<P> Clone for BoundlessMarketService<P>
where
    IBoundlessMarketInstance<(), P, Ethereum>: Clone,
{
    fn clone(&self) -> Self {
        Self {
            instance: self.instance.clone(),
            chain_id: self.chain_id.load(Ordering::Relaxed).into(),
            caller: self.caller,
            timeout: self.timeout,
            event_query_config: self.event_query_config.clone(),
            balance_alert_config: self.balance_alert_config.clone(),
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
        [] => Err(anyhow!("transaction did not emit event {}", E::SIGNATURE)),
        _ => Err(anyhow!(
            "transaction emitted more than one event with signature {}, {:#?}",
            E::SIGNATURE,
            logs
        )),
    }
}

impl<P: Provider> BoundlessMarketService<P> {
    /// Creates a new Boundless market service.
    pub fn new(address: Address, provider: P, caller: Address) -> Self {
        let instance = IBoundlessMarket::new(address, provider);

        Self {
            instance,
            chain_id: AtomicU64::new(0),
            caller,
            timeout: TXN_CONFIRM_TIMEOUT,
            event_query_config: EventQueryConfig::default(),
            balance_alert_config: StakeBalanceAlertConfig::default(),
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

    /// Set stake balance thresholds to warn or error alert on
    pub fn with_stake_balance_alert(
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

    /// Returns the market contract instance.
    pub fn instance(&self) -> &IBoundlessMarketInstance<(), P, Ethereum> {
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
        Ok(U256::from(log.inner.data.requestId))
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
    /// auction parameters and the block number at which this transaction is processed.
    ///
    /// This method should be called from the address of the prover.
    pub async fn lock_request(
        &self,
        request: &ProofRequest,
        client_sig: &Bytes,
        priority_gas: Option<u64>,
    ) -> Result<u64, MarketError> {
        tracing::debug!("Calling requestIsLocked({:x})", request.id);
        let is_locked_in: bool =
            self.instance.requestIsLocked(request.id).call().await.context("call failed")?._0;
        if is_locked_in {
            return Err(MarketError::Error(anyhow!("request is already locked")));
        }

        tracing::debug!("Calling lockRequest({:x?}, {:x?})", request, client_sig);

        let mut call =
            self.instance.lockRequest(request.clone(), client_sig.clone()).from(self.caller);

        if let Some(gas) = priority_gas {
            let priority_fee = self
                .instance
                .provider()
                .estimate_eip1559_fees()
                .await
                .context("Failed to get priority gas fee")?;

            call = call
                .max_fee_per_gas(priority_fee.max_fee_per_gas + gas as u128)
                .max_priority_fee_per_gas(priority_fee.max_priority_fee_per_gas + gas as u128);
        }

        tracing::debug!("Sending tx {}", format!("{:?}", call));

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

        self.check_stake_balance().await?;

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
        client_sig: &Bytes,
        prover_address: Address,
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
            "Calling lockRequestWithSignature({:x?}, {:x?}, {:x?}, {:x?})",
            request,
            client_sig,
            prover_address,
            prover_sig
        );

        let call = self
            .instance
            .lockRequestWithSignature(request.clone(), client_sig.clone(), prover_sig.clone())
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
                "lockRequestWithSignature failed [{}], possibly outbid",
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
    pub async fn fulfill(
        &self,
        fulfillment: &Fulfillment,
        assessor_fill: AssessorReceipt,
    ) -> Result<(), MarketError> {
        tracing::debug!("Calling fulfill({:x?},{:x?})", fulfillment, assessor_fill);
        let call = self.instance.fulfill(fulfillment.clone(), assessor_fill).from(self.caller);
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

        Ok(())
    }

    /// Fulfill a request by delivering the proof for the application and withdraw from the prover balance.
    ///
    /// Upon proof verification, the prover is paid as long as the requirements are met, including:
    ///
    /// * Seal for the assessor proof is valid, verifying that the order's requirements are met.
    /// * The order has not expired.
    /// * The order is not locked by a different prover.
    /// * A prover has not been paid for the job already.
    /// * If not locked, the client has sufficient funds.
    pub async fn fulfill_and_withdraw(
        &self,
        fulfillment: &Fulfillment,
        assessor_fill: AssessorReceipt,
    ) -> Result<(), MarketError> {
        tracing::debug!("Calling fulfillAndWithdraw({:x?},{:x?})", fulfillment, assessor_fill);
        let call =
            self.instance.fulfillAndWithdraw(fulfillment.clone(), assessor_fill).from(self.caller);
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

        Ok(())
    }

    /// Fulfill a batch of requests by delivering the proof for each application.
    ///
    /// See [BoundlessMarketService::fulfill] for more details.
    pub async fn fulfill_batch(
        &self,
        fulfillments: Vec<Fulfillment>,
        assessor_fill: AssessorReceipt,
    ) -> Result<(), MarketError> {
        let fill_ids = fulfillments.iter().map(|fill| fill.id).collect::<Vec<_>>();
        tracing::debug!("Calling fulfillBatch({fulfillments:?}, {assessor_fill:?})");
        let call = self.instance.fulfillBatch(fulfillments, assessor_fill).from(self.caller);
        tracing::debug!("Calldata: {:x}", call.calldata());
        let pending_tx = call.send().await?;
        tracing::debug!("Broadcasting tx {}", pending_tx.tx_hash());

        let receipt = pending_tx
            .with_timeout(Some(self.timeout))
            .get_receipt()
            .await
            .context("failed to confirm tx")?;

        tracing::info!("Submitted proof for batch {:?}: {}", fill_ids, receipt.transaction_hash);

        Ok(())
    }

    /// Fulfill a batch of requests by delivering the proof for each application and withdraw from the prover balance.
    ///
    /// See [BoundlessMarketService::fulfill] for more details.
    pub async fn fulfill_batch_and_withdraw(
        &self,
        fulfillments: Vec<Fulfillment>,
        assessor_fill: AssessorReceipt,
    ) -> Result<(), MarketError> {
        let fill_ids = fulfillments.iter().map(|fill| fill.id).collect::<Vec<_>>();
        tracing::debug!("Calling fulfillBatchAndWithdraw({fulfillments:?}, {assessor_fill:?})");
        let call =
            self.instance.fulfillBatchAndWithdraw(fulfillments, assessor_fill).from(self.caller);
        tracing::debug!("Calldata: {:x}", call.calldata());
        let pending_tx = call.send().await?;
        tracing::debug!("Broadcasting tx {}", pending_tx.tx_hash());

        let receipt = pending_tx
            .with_timeout(Some(self.timeout))
            .get_receipt()
            .await
            .context("failed to confirm tx")?;

        tracing::info!("Submitted proof for batch {:?}: {}", fill_ids, receipt.transaction_hash);

        Ok(())
    }

    /// Combined function to submit a new merkle root to the set-verifier and call `fulfillBatch`.
    /// Useful to reduce the transaction count for fulfillments
    pub async fn submit_merkle_and_fulfill(
        &self,
        verifier_address: Address,
        root: B256,
        seal: Bytes,
        fulfillments: Vec<Fulfillment>,
        assessor_fill: AssessorReceipt,
    ) -> Result<(), MarketError> {
        tracing::debug!("Calling submitRootAndFulfillBatch({root:?}, {seal:x}, {fulfillments:?}, {assessor_fill:?})");
        let call = self
            .instance
            .submitRootAndFulfillBatch(verifier_address, root, seal, fulfillments, assessor_fill)
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

    /// Combined function to submit a new merkle root to the set-verifier and call `fulfillBatchAndWithdraw`.
    /// Useful to reduce the transaction count for fulfillments
    pub async fn submit_merkle_and_fulfill_and_withdraw(
        &self,
        verifier_address: Address,
        root: B256,
        seal: Bytes,
        fulfillments: Vec<Fulfillment>,
        assessor_fill: AssessorReceipt,
    ) -> Result<(), MarketError> {
        tracing::debug!("Calling submitRootAndFulfillBatchAndWithdraw({root:?}, {seal:x}, {fulfillments:?}, {assessor_fill:?})");
        let call = self
            .instance
            .submitRootAndFulfillBatchAndWithdraw(
                verifier_address,
                root,
                seal,
                fulfillments,
                assessor_fill,
            )
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
        assessor_fill: AssessorReceipt,
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

        tracing::debug!("Calling priceAndFulfillBatch({fulfillments:?}, {assessor_fill:?})");

        let mut call = self
            .instance
            .priceAndFulfillBatch(requests, client_sigs, fulfillments, assessor_fill)
            .from(self.caller);
        tracing::debug!("Calldata: {}", call.calldata());

        if let Some(gas) = priority_gas {
            let priority_fee = self
                .instance
                .provider()
                .estimate_eip1559_fees()
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

    /// A combined call to `IBoundlessMarket.priceRequest` and `IBoundlessMarket.fulfillBatchAndWithdraw`.
    /// The caller should provide the signed request and signature for each unlocked request they
    /// want to fulfill. Payment for unlocked requests will go to the provided `prover` address.
    pub async fn price_and_fulfill_batch_and_withdraw(
        &self,
        requests: Vec<ProofRequest>,
        client_sigs: Vec<Bytes>,
        fulfillments: Vec<Fulfillment>,
        assessor_fill: AssessorReceipt,
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

        tracing::debug!(
            "Calling priceAndFulfillBatchAndWithdraw({fulfillments:?}, {assessor_fill:?})"
        );

        let mut call = self
            .instance
            .priceAndFulfillBatchAndWithdraw(requests, client_sigs, fulfillments, assessor_fill)
            .from(self.caller);
        tracing::debug!("Calldata: {}", call.calldata());

        if let Some(gas) = priority_gas {
            let priority_fee = self
                .instance
                .provider()
                .estimate_eip1559_fees()
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
    pub async fn is_locked(&self, request_id: U256) -> Result<bool, MarketError> {
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
            let deadline = self.instance.requestDeadline(request_id).call().await?._0;
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
    ) -> Result<(Bytes, Bytes), MarketError> {
        let mut upper_block = upper_bound.unwrap_or(self.get_latest_block_number().await?);
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

            if let Some((_, data)) = logs.first() {
                // get the calldata inputs
                let tx_data = self
                    .instance
                    .provider()
                    .get_transaction_by_hash(data.transaction_hash.context("tx hash is none")?)
                    .await
                    .context("Failed to get transaction")?
                    .context("Transaction not found")?;
                let inputs = tx_data.input();
                let fills = decode_calldata(inputs).context("Failed to decode calldata")?;
                for fill in fills {
                    if fill.id == request_id {
                        return Ok((fill.journal.clone(), fill.seal.clone()));
                    }
                }
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
    /// The default range is set to 1000 blocks for each iteration, and the default maximum number of
    /// iterations is 100. This means that the search will cover a maximum of 100,000 blocks.
    /// Optionally, you can specify a lower and upper bound to limit the search range.
    async fn query_request_submitted_event(
        &self,
        request_id: U256,
        lower_bound: Option<u64>,
        upper_bound: Option<u64>,
    ) -> Result<(ProofRequest, Bytes), MarketError> {
        let mut upper_block = upper_bound.unwrap_or(self.get_latest_block_number().await?);
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

            if let Some((_, data)) = logs.first() {
                // get the calldata inputs
                let tx_data = self
                    .instance
                    .provider()
                    .get_transaction_by_hash(data.transaction_hash.context("tx hash is none")?)
                    .await
                    .context("Failed to get transaction")?
                    .context("Transaction not found")?;
                let inputs = tx_data.input();
                let calldata = IBoundlessMarket::submitRequestCall::abi_decode(inputs, true)
                    .context("Failed to decode input")?;
                return Ok((calldata.request, calldata.clientSignature));
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
            RequestStatus::Expired => Err(MarketError::RequestHasExpired(request_id)),
            RequestStatus::Fulfilled => self.query_fulfilled_event(request_id, None, None).await,
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
            let tx_data = self
                .instance
                .provider()
                .get_transaction_by_hash(tx_hash)
                .await
                .context("Failed to get transaction")?
                .context("Transaction not found")?;
            let inputs = tx_data.input();
            let calldata = IBoundlessMarket::submitRequestCall::abi_decode(inputs, true)
                .context("Failed to decode input")?;
            return Ok((calldata.request, calldata.clientSignature));
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
                RequestStatus::Expired => return Err(MarketError::RequestHasExpired(request_id)),
                RequestStatus::Fulfilled => {
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
        self.check_stake_balance().await?;
        Ok(())
    }

    /// Returns the deposited balance, in HP, of the given account.
    pub async fn balance_of_stake(&self, account: Address) -> Result<U256, MarketError> {
        tracing::debug!("Calling balanceOfStake({})", account);
        let balance = self.instance.balanceOfStake(account).call().await.context("call failed")?._0;
        Ok(balance)
    }

    /// Check the current stake balance against the alert config
    /// and log a warning or error or below the thresholds.
    async fn check_stake_balance(&self) -> Result<(), MarketError> {
        let stake_balance = self.balance_of_stake(self.caller()).await?;
        if stake_balance < self.balance_alert_config.error_threshold.unwrap_or(U256::ZERO) {
            tracing::error!(
                "stake balance {} for {} < error threshold",
                stake_balance,
                self.caller(),
            );
        } else if stake_balance < self.balance_alert_config.warn_threshold.unwrap_or(U256::ZERO) {
            tracing::warn!(
                "stake balance {} for {} < warning threshold",
                stake_balance,
                self.caller(),
            );
        } else {
            tracing::trace!("stake balance for {} is: {}", self.caller(), stake_balance);
        }
        Ok(())
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

        Ok(self.biddingStart + delta)
    }

    /// Calculates the price at the given time, in seconds since the UNIX epoch.
    pub fn price_at(&self, timestamp: u64) -> Result<U256, MarketError> {
        let max_price = U256::from(self.maxPrice);
        let min_price = U256::from(self.minPrice);

        if timestamp < self.biddingStart {
            return Ok(self.minPrice);
        }

        if timestamp > self.lock_deadline() {
            return Ok(U256::ZERO);
        }

        if timestamp < self.biddingStart + self.rampUpPeriod as u64 {
            let rise = max_price - min_price;
            let run = U256::from(self.rampUpPeriod);
            let delta = U256::from(timestamp) - U256::from(self.biddingStart);

            Ok(min_price + (delta * rise) / run)
        } else {
            Ok(max_price)
        }
    }

    /// UNIX timestamp after which the request is considered completely expired.
    pub fn deadline(&self) -> u64 {
        self.biddingStart + (self.timeout as u64)
    }

    /// UNIX timestamp after which any lock on the request expires, and the client fee is zero.
    ///
    /// Once locked, if a valid proof is not submitted before this deadline, the prover can be
    /// "slashed", which refunds the price to the requester and takes the prover stake.
    /// Additionally, the fee paid by the client is zero for proofs delivered after this time. Note
    /// that after this time, and before `timeout` a proof can still be delivered to fulfill the
    /// request.
    pub fn lock_deadline(&self) -> u64 {
        self.biddingStart + (self.lockTimeout as u64)
    }
}

fn decode_calldata(data: &Bytes) -> Result<Vec<Fulfillment>> {
    if let Ok(call) = IBoundlessMarket::submitRootAndFulfillBatchCall::abi_decode(data, true) {
        return Ok(call.fills);
    }
    if let Ok(call) =
        IBoundlessMarket::submitRootAndFulfillBatchAndWithdrawCall::abi_decode(data, true)
    {
        return Ok(call.fills);
    }
    if let Ok(call) = IBoundlessMarket::fulfillCall::abi_decode(data, true) {
        return Ok(vec![call.fill]);
    }
    if let Ok(call) = IBoundlessMarket::fulfillBatchCall::abi_decode(data, true) {
        return Ok(call.fills);
    }
    if let Ok(call) = IBoundlessMarket::fulfillAndWithdrawCall::abi_decode(data, true) {
        return Ok(vec![call.fill]);
    }
    if let Ok(call) = IBoundlessMarket::fulfillBatchAndWithdrawCall::abi_decode(data, true) {
        return Ok(call.fills);
    }
    if let Ok(call) = IBoundlessMarket::priceAndFulfillCall::abi_decode(data, true) {
        return Ok(vec![call.fill]);
    }
    if let Ok(call) = IBoundlessMarket::priceAndFulfillBatchCall::abi_decode(data, true) {
        return Ok(call.fills);
    }
    if let Ok(call) = IBoundlessMarket::priceAndFulfillAndWithdrawCall::abi_decode(data, true) {
        return Ok(vec![call.fill]);
    }
    if let Ok(call) = IBoundlessMarket::priceAndFulfillBatchAndWithdrawCall::abi_decode(data, true)
    {
        return Ok(call.fills);
    }

    Err(anyhow!(
        "Failed to decode calldata with selector {} as any fulfillment call",
        hex::encode(&data[0..4])
    ))
}

#[cfg(test)]
mod tests {
    use super::decode_calldata;
    use crate::{
        contracts::{
            hit_points::default_allowance,
            test_utils::{create_test_ctx, TestCtx},
            AssessorCommitment, AssessorJournal, AssessorReceipt, Fulfillment, IBoundlessMarket,
            Input, InputType, Offer, Predicate, PredicateType, ProofRequest, RequestStatus,
            Requirements,
        },
        input::InputBuilder,
        now_timestamp,
    };
    use alloy::{
        consensus::Transaction,
        node_bindings::Anvil,
        primitives::{aliases::U160, utils::parse_ether, Address, Bytes, B256, U256},
        providers::Provider,
        sol_types::{eip712_domain, Eip712Domain, SolStruct, SolValue},
    };
    use alloy_sol_types::SolCall;
    use guest_assessor::{ASSESSOR_GUEST_ID, ASSESSOR_GUEST_PATH};
    use guest_set_builder::{SET_BUILDER_ID, SET_BUILDER_PATH};
    use guest_util::ECHO_ID;
    use risc0_aggregation::{
        merkle_root, GuestState, SetInclusionReceipt, SetInclusionReceiptVerifierParameters,
    };
    use risc0_ethereum_contracts::encode_seal;
    use risc0_zkvm::{
        sha::{Digest, Digestible},
        FakeReceipt, InnerReceipt, Journal, MaybePruned, Receipt, ReceiptClaim,
    };
    use tracing_test::traced_test;

    fn ether(value: &str) -> U256 {
        parse_ether(value).unwrap()
    }

    fn test_offer(bidding_start: u64) -> Offer {
        Offer {
            minPrice: ether("1"),
            maxPrice: ether("2"),
            biddingStart: bidding_start,
            rampUpPeriod: 100,
            timeout: 500,
            lockTimeout: 500,
            lockStake: ether("1"),
        }
    }

    async fn new_request<P: Provider>(idx: u32, ctx: &TestCtx<P>) -> ProofRequest {
        ProofRequest::new(
            idx,
            &ctx.customer_signer.address(),
            Requirements::new(
                Digest::from(ECHO_ID),
                Predicate { predicateType: PredicateType::PrefixMatch, data: Default::default() },
            ),
            "http://image_uri.null",
            InputBuilder::new().build_inline().unwrap(),
            Offer {
                minPrice: U256::from(20000000000000u64),
                maxPrice: U256::from(40000000000000u64),
                biddingStart: now_timestamp(),
                timeout: 100,
                rampUpPeriod: 1,
                lockStake: U256::from(10),
                lockTimeout: 100,
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
        let request_digest = request.eip712_signing_hash(&eip712_domain);
        let assessor_root = AssessorCommitment {
            index: U256::ZERO,
            id: request.id,
            requestDigest: request_digest,
            claimDigest: <[u8; 32]>::from(app_claim_digest).into(),
        }
        .eip712_hash_struct();
        let assessor_journal =
            AssessorJournal { selectors: vec![], root: assessor_root, prover, callbacks: vec![] };
        let assesor_receipt_claim =
            ReceiptClaim::ok(ASSESSOR_GUEST_ID, assessor_journal.abi_encode());
        let assessor_claim_digest = assesor_receipt_claim.digest();

        let set_builder_root = merkle_root(&[app_claim_digest, assessor_claim_digest]);
        let set_builder_journal = {
            let mut state = GuestState::initial(SET_BUILDER_ID);
            state.mmr.push(set_builder_root).unwrap();
            state.mmr.finalize().unwrap();
            state.encode()
        };
        let set_builder_receipt_claim =
            ReceiptClaim::ok(SET_BUILDER_ID, set_builder_journal.clone());

        let set_builder_receipt = Receipt::new(
            InnerReceipt::Fake(FakeReceipt::new(set_builder_receipt_claim)),
            set_builder_journal,
        );
        let set_builder_seal = encode_seal(&set_builder_receipt).unwrap();

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
            requestDigest: request_digest,
            imageId: to_b256(Digest::from(ECHO_ID)),
            journal: app_journal.bytes.into(),
            seal: set_inclusion_seal.into(),
        };

        let assessor_seal = SetInclusionReceipt::from_path_with_verifier_params(
            ReceiptClaim::ok(ASSESSOR_GUEST_ID, MaybePruned::Pruned(Digest::ZERO)),
            vec![app_claim_digest],
            verifier_parameters.digest(),
        )
        .abi_encode_seal()
        .unwrap();

        (to_b256(set_builder_root), set_builder_seal.into(), fulfillment, assessor_seal.into())
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

    #[tokio::test]
    async fn test_deposit_withdraw() {
        // Setup anvil
        let anvil = Anvil::new().spawn();

        let ctx = create_test_ctx(
            &anvil,
            SET_BUILDER_ID,
            format!("file://{SET_BUILDER_PATH}"),
            ASSESSOR_GUEST_ID,
            format!("file://{ASSESSOR_GUEST_PATH}"),
        )
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
    #[traced_test]
    async fn test_deposit_withdraw_stake() {
        // Setup anvil
        let anvil = Anvil::new().spawn();

        let mut ctx = create_test_ctx(
            &anvil,
            SET_BUILDER_ID,
            format!("file://{SET_BUILDER_PATH}"),
            ASSESSOR_GUEST_ID,
            format!("file://{ASSESSOR_GUEST_PATH}"),
        )
        .await
        .unwrap();

        let deposit = U256::from(10);

        // set stake balance alerts
        ctx.prover_market =
            ctx.prover_market.with_stake_balance_alert(&Some(U256::from(10)), &Some(U256::from(5)));

        // Approve and deposit stake
        ctx.prover_market.approve_deposit_stake(deposit).await.unwrap();
        ctx.prover_market.deposit_stake(deposit).await.unwrap();

        // Deposit stake with permit
        ctx.prover_market.deposit_stake_with_permit(deposit, &ctx.prover_signer).await.unwrap();

        assert_eq!(
            ctx.prover_market.balance_of_stake(ctx.prover_signer.address()).await.unwrap(),
            U256::from(20)
        );

        // Withdraw prover balances in chunks to observe alerts

        ctx.prover_market.withdraw_stake(U256::from(11)).await.unwrap();
        assert_eq!(
            ctx.prover_market.balance_of_stake(ctx.prover_signer.address()).await.unwrap(),
            U256::from(9)
        );
        assert!(logs_contain("< warning threshold"));

        ctx.prover_market.withdraw_stake(U256::from(5)).await.unwrap();
        assert_eq!(
            ctx.prover_market.balance_of_stake(ctx.prover_signer.address()).await.unwrap(),
            U256::from(4)
        );
        assert!(logs_contain("< error threshold"));

        ctx.prover_market.withdraw_stake(U256::from(4)).await.unwrap();
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

        let ctx = create_test_ctx(
            &anvil,
            SET_BUILDER_ID,
            format!("file://{SET_BUILDER_PATH}"),
            ASSESSOR_GUEST_ID,
            format!("file://{ASSESSOR_GUEST_PATH}"),
        )
        .await
        .unwrap();

        let request = new_request(1, &ctx).await;

        let request_id =
            ctx.customer_market.submit_request(&request, &ctx.customer_signer).await.unwrap();

        // fetch logs and check if the event was emitted
        let logs = ctx.customer_market.instance().RequestSubmitted_filter().query().await.unwrap();

        let (log, _) = logs.first().unwrap();
        assert!(log.requestId == request_id);
    }

    #[tokio::test]
    #[traced_test]
    async fn test_e2e() {
        // Setup anvil
        let anvil = Anvil::new().spawn();

        let ctx = create_test_ctx(
            &anvil,
            SET_BUILDER_ID,
            format!("file://{SET_BUILDER_PATH}"),
            ASSESSOR_GUEST_ID,
            format!("file://{ASSESSOR_GUEST_PATH}"),
        )
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
        let tx_hash = log.transaction_hash.unwrap();
        let tx_data = ctx
            .customer_market
            .instance()
            .provider()
            .get_transaction_by_hash(tx_hash)
            .await
            .unwrap()
            .unwrap();
        let inputs = tx_data.input();
        let calldata = IBoundlessMarket::submitRequestCall::abi_decode(inputs, true).unwrap();

        let request = calldata.request;
        let customer_sig = calldata.clientSignature;

        // Deposit prover balances
        let deposit = default_allowance();
        ctx.prover_market.deposit_stake_with_permit(deposit, &ctx.prover_signer).await.unwrap();

        // Lock the request
        ctx.prover_market.lock_request(&request, &customer_sig, None).await.unwrap();
        assert!(ctx.customer_market.is_locked(request_id).await.unwrap());
        assert!(
            ctx.customer_market.get_status(request_id, Some(expires_at)).await.unwrap()
                == RequestStatus::Locked
        );

        // mock the fulfillment
        let (root, set_verifier_seal, fulfillment, assessor_seal) =
            mock_singleton(&request, eip712_domain, ctx.prover_signer.address());

        // publish the committed root
        ctx.set_verifier.submit_merkle_root(root, set_verifier_seal).await.unwrap();

        let assessor_fill = AssessorReceipt {
            seal: assessor_seal,
            selectors: vec![],
            prover: ctx.prover_signer.address(),
            callbacks: vec![],
        };
        // fulfill the request
        ctx.prover_market.fulfill(&fulfillment, assessor_fill).await.unwrap();
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

        let ctx = create_test_ctx(
            &anvil,
            SET_BUILDER_ID,
            format!("file://{SET_BUILDER_PATH}"),
            ASSESSOR_GUEST_ID,
            format!("file://{ASSESSOR_GUEST_PATH}"),
        )
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
        let tx_hash = log.transaction_hash.unwrap();
        let tx_data = ctx
            .customer_market
            .instance()
            .provider()
            .get_transaction_by_hash(tx_hash)
            .await
            .unwrap()
            .unwrap();
        let inputs = tx_data.input();
        let calldata = IBoundlessMarket::submitRequestCall::abi_decode(inputs, true).unwrap();

        let request = calldata.request;
        let customer_sig = calldata.clientSignature;

        // Deposit prover balances
        let deposit = default_allowance();
        ctx.prover_market.deposit_stake_with_permit(deposit, &ctx.prover_signer).await.unwrap();

        // Lock the request
        ctx.prover_market.lock_request(&request, &customer_sig, None).await.unwrap();
        assert!(ctx.customer_market.is_locked(request_id).await.unwrap());
        assert!(
            ctx.customer_market.get_status(request_id, Some(expires_at)).await.unwrap()
                == RequestStatus::Locked
        );

        // mock the fulfillment
        let (root, set_verifier_seal, fulfillment, assessor_seal) =
            mock_singleton(&request, eip712_domain, ctx.prover_signer.address());

        let fulfillments = vec![fulfillment];
        let assessor_fill = AssessorReceipt {
            seal: assessor_seal,
            selectors: vec![],
            prover: ctx.prover_signer.address(),
            callbacks: vec![],
        };
        // publish the committed root + fulfillments
        ctx.prover_market
            .submit_merkle_and_fulfill(
                ctx.set_verifier_address,
                root,
                set_verifier_seal,
                fulfillments.clone(),
                assessor_fill,
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

        let ctx = create_test_ctx(
            &anvil,
            SET_BUILDER_ID,
            format!("file://{SET_BUILDER_PATH}"),
            ASSESSOR_GUEST_ID,
            format!("file://{ASSESSOR_GUEST_PATH}"),
        )
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
        let tx_hash = log.transaction_hash.unwrap();
        let tx_data = ctx
            .customer_market
            .instance()
            .provider()
            .get_transaction_by_hash(tx_hash)
            .await
            .unwrap()
            .unwrap();
        let inputs = tx_data.input();
        let calldata = IBoundlessMarket::submitRequestCall::abi_decode(inputs, true).unwrap();

        let request = calldata.request;
        let customer_sig = calldata.clientSignature;

        // mock the fulfillment
        let (root, set_verifier_seal, fulfillment, assessor_seal) =
            mock_singleton(&request, eip712_domain, ctx.prover_signer.address());

        let fulfillments = vec![fulfillment];
        let assessor_fill = AssessorReceipt {
            seal: assessor_seal,
            selectors: vec![],
            prover: ctx.prover_signer.address(),
            callbacks: vec![],
        };
        // publish the committed root
        ctx.set_verifier.submit_merkle_root(root, set_verifier_seal).await.unwrap();

        // Price and fulfill the request
        ctx.prover_market
            .price_and_fulfill_batch(
                vec![request],
                vec![customer_sig],
                fulfillments.clone(),
                assessor_fill,
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
    async fn test_e2e_no_payment() {
        // Setup anvil
        let anvil = Anvil::new().spawn();

        let ctx = create_test_ctx(
            &anvil,
            SET_BUILDER_ID,
            format!("file://{SET_BUILDER_PATH}"),
            ASSESSOR_GUEST_ID,
            format!("file://{ASSESSOR_GUEST_PATH}"),
        )
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
        let tx_hash = log.transaction_hash.unwrap();
        let tx_data = ctx
            .customer_market
            .instance()
            .provider()
            .get_transaction_by_hash(tx_hash)
            .await
            .unwrap()
            .unwrap();
        let inputs = tx_data.input();
        let calldata = IBoundlessMarket::submitRequestCall::abi_decode(inputs, true).unwrap();

        let request = calldata.request;
        let customer_sig = calldata.clientSignature;

        // Deposit prover balances
        let deposit = default_allowance();
        ctx.prover_market.deposit_stake_with_permit(deposit, &ctx.prover_signer).await.unwrap();

        // Lock the request
        ctx.prover_market.lock_request(&request, &customer_sig, None).await.unwrap();
        assert!(ctx.customer_market.is_locked(request_id).await.unwrap());
        assert!(
            ctx.customer_market.get_status(request_id, Some(expires_at)).await.unwrap()
                == RequestStatus::Locked
        );

        // Test behavior when payment requirements are not met.
        {
            // mock the fulfillment, using the wrong prover address. Address::from(3) arbitrary.
            let some_other_address = Address::from(U160::from(3));
            let (root, set_verifier_seal, fulfillment, assessor_seal) =
                mock_singleton(&request, eip712_domain.clone(), some_other_address);

            // publish the committed root
            ctx.set_verifier.submit_merkle_root(root, set_verifier_seal).await.unwrap();

            let assessor_fill = AssessorReceipt {
                seal: assessor_seal,
                selectors: vec![],
                prover: some_other_address,
                callbacks: vec![],
            };

            let balance_before = ctx.prover_market.balance_of(some_other_address).await.unwrap();
            // fulfill the request.
            ctx.prover_market.fulfill(&fulfillment, assessor_fill.clone()).await.unwrap();
            assert!(ctx.customer_market.is_fulfilled(request_id).await.unwrap());
            let balance_after = ctx.prover_market.balance_of(some_other_address).await.unwrap();
            assert!(balance_before == balance_after);

            // retrieve journal and seal from the fulfilled request
            let (journal, seal) =
                ctx.customer_market.get_request_fulfillment(request_id).await.unwrap();

            assert_eq!(journal, fulfillment.journal);
            assert_eq!(seal, fulfillment.seal);
        }

        // mock the fulfillment, this time using the right prover address.
        let (root, set_verifier_seal, fulfillment, assessor_seal) =
            mock_singleton(&request, eip712_domain, ctx.prover_signer.address());

        // publish the committed root
        ctx.set_verifier.submit_merkle_root(root, set_verifier_seal).await.unwrap();

        let assessor_fill = AssessorReceipt {
            seal: assessor_seal,
            selectors: vec![],
            prover: ctx.prover_signer.address(),
            callbacks: vec![],
        };

        // fulfill the request, this time getting paid.
        ctx.prover_market.fulfill(&fulfillment, assessor_fill).await.unwrap();
        assert!(ctx.customer_market.is_fulfilled(request_id).await.unwrap());

        // retrieve journal and seal from the fulfilled request
        let (_journal, _seal) =
            ctx.customer_market.get_request_fulfillment(request_id).await.unwrap();

        // TODO: Instead of checking that this is the same seal, check if this is some valid seal.
        // When there are multiple fulfillments one order, there will be multiple ProofDelivered
        // events. All proofs will be valid though.
        //assert_eq!(journal, fulfillment.journal);
        //assert_eq!(seal, fulfillment.seal);
    }

    #[test]
    fn test_decode_calldata() {
        let request = ProofRequest::new(
            0,
            &Address::ZERO,
            Requirements::new(
                Digest::ZERO,
                Predicate { predicateType: PredicateType::PrefixMatch, data: Default::default() },
            ),
            "http:s//image.dev.null",
            Input { inputType: InputType::Inline, data: Default::default() },
            Offer {
                minPrice: U256::from(1),
                maxPrice: U256::from(4),
                biddingStart: now_timestamp(),
                timeout: 100,
                rampUpPeriod: 1,
                lockTimeout: 100,
                lockStake: U256::from(10),
            },
        );

        let fill = Fulfillment {
            id: U256::default(),
            requestDigest: B256::default(),
            imageId: B256::default(),
            journal: Bytes::from(vec![1, 2, 3]),
            seal: Bytes::from(vec![1, 2, 3]),
        };
        let assessor_receipt = AssessorReceipt {
            seal: Bytes::from(vec![1, 2, 3]),
            selectors: vec![],
            prover: Address::from(U160::from(1)),
            callbacks: vec![],
        };

        let call = IBoundlessMarket::submitRootAndFulfillBatchCall {
            setVerifier: Address::default(),
            root: B256::default(),
            seal: Bytes::default(),
            fills: vec![fill.clone()],
            assessorReceipt: assessor_receipt.clone(),
        };
        decode_calldata(&call.abi_encode().into()).unwrap();

        let call = IBoundlessMarket::submitRootAndFulfillBatchAndWithdrawCall {
            setVerifier: Address::default(),
            root: B256::default(),
            seal: Bytes::default(),
            fills: vec![fill.clone()],
            assessorReceipt: assessor_receipt.clone(),
        };
        decode_calldata(&call.abi_encode().into()).unwrap();

        let call = IBoundlessMarket::fulfillCall {
            fill: fill.clone(),
            assessorReceipt: assessor_receipt.clone(),
        };
        decode_calldata(&call.abi_encode().into()).unwrap();

        let call = IBoundlessMarket::fulfillBatchCall {
            fills: vec![fill.clone()],
            assessorReceipt: assessor_receipt.clone(),
        };
        decode_calldata(&call.abi_encode().into()).unwrap();

        let call = IBoundlessMarket::fulfillAndWithdrawCall {
            fill: fill.clone(),
            assessorReceipt: assessor_receipt.clone(),
        };
        decode_calldata(&call.abi_encode().into()).unwrap();

        let call = IBoundlessMarket::fulfillBatchAndWithdrawCall {
            fills: vec![fill.clone()],
            assessorReceipt: assessor_receipt.clone(),
        };
        decode_calldata(&call.abi_encode().into()).unwrap();

        let call = IBoundlessMarket::priceAndFulfillCall {
            request: request.clone(),
            clientSignature: Bytes::default(),
            fill: fill.clone(),
            assessorReceipt: assessor_receipt.clone(),
        };
        decode_calldata(&call.abi_encode().into()).unwrap();

        let call = IBoundlessMarket::priceAndFulfillBatchCall {
            requests: vec![request.clone()],
            clientSignatures: vec![Bytes::default()],
            fills: vec![fill.clone()],
            assessorReceipt: assessor_receipt.clone(),
        };
        decode_calldata(&call.abi_encode().into()).unwrap();

        let call = IBoundlessMarket::priceAndFulfillAndWithdrawCall {
            request: request.clone(),
            clientSignature: Bytes::default(),
            fill: fill.clone(),
            assessorReceipt: assessor_receipt.clone(),
        };
        decode_calldata(&call.abi_encode().into()).unwrap();

        let call = IBoundlessMarket::priceAndFulfillBatchAndWithdrawCall {
            requests: vec![request.clone()],
            clientSignatures: vec![Bytes::default()],
            fills: vec![fill.clone()],
            assessorReceipt: assessor_receipt.clone(),
        };
        decode_calldata(&call.abi_encode().into()).unwrap();
    }
}
