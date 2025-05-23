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

use alloy::{
    network::Ethereum,
    primitives::Address,
    providers::{PendingTransactionBuilder, Provider, ProviderLayer, RootProvider},
    rpc::types::TransactionRequest,
    transports::TransportResult,
};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{Mutex, Semaphore};

#[derive(Clone, Default)]
/// A layer that manages nonces per account using semaphores.
///
/// It fetches the pending nonce before each transaction and holds a semaphore
/// until the transaction is submitted to ensure nonces are managed correctly.
pub struct NonceLayer {
    account_semaphores: Arc<Mutex<HashMap<Address, Arc<Semaphore>>>>,
}

impl<P> ProviderLayer<P> for NonceLayer
where
    P: Provider<Ethereum>,
{
    type Provider = NonceProvider<P>;

    fn layer(&self, inner: P) -> Self::Provider {
        NonceProvider {
            inner: Arc::new(inner),
            account_semaphores: Arc::clone(&self.account_semaphores),
        }
    }
}

#[derive(Clone)]
/// A provider that manages nonces per account using semaphores.
pub struct NonceProvider<P> {
    inner: Arc<P>,
    account_semaphores: Arc<Mutex<HashMap<Address, Arc<Semaphore>>>>,
}

impl<P> NonceProvider<P>
where
    P: Provider<Ethereum> + Send + Sync,
{
    /// Get or create a semaphore for the given account address.
    async fn get_account_semaphore(&self, address: Address) -> Arc<Semaphore> {
        let mut semaphores = self.account_semaphores.lock().await;
        semaphores
            .entry(address)
            .or_insert_with(|| Arc::new(Semaphore::new(1)))
            .clone()
    }

    /// Send a transaction with proper nonce management.
    async fn send_with_nonce_management(
        &self,
        mut request: TransactionRequest,
    ) -> TransportResult<PendingTransactionBuilder<Ethereum>> {
        // Extract the from address from the transaction request
        let from_address = request.from.ok_or_else(|| {
            alloy::transports::RpcError::Transport(
                alloy::transports::TransportErrorKind::Custom(
                    "Transaction request must have a 'from' address for nonce management".into()
                )
            )
        })?;

        let semaphore = self.get_account_semaphore(from_address).await;

        // Acquire semaphore to ensure only one transaction per account at a time
        let _permit = semaphore.acquire().await.unwrap();
        tracing::trace!(
            "NonceProvider::send_with_nonce_management - acquired semaphore for address: {}",
            from_address
        );

        // Fetch the pending nonce if not already set
        if request.nonce.is_none() {
            let pending_nonce = self
                .inner
                .get_transaction_count(from_address)
                .pending()
                .await?;
            request.nonce = Some(pending_nonce);
            tracing::trace!(
                "NonceProvider::send_with_nonce_management - set nonce {} for address: {}",
                pending_nonce,
                from_address
            );
        }

        // Send the transaction
        let pending_tx = self.inner.send_transaction(request).await?;
        tracing::trace!(
            "NonceProvider::send_with_nonce_management - transaction sent: {:?}",
            pending_tx.tx_hash()
        );

        // Semaphore is automatically released when _permit is dropped
        Ok(pending_tx)
    }
}

#[async_trait::async_trait]
impl<P> Provider<Ethereum> for NonceProvider<P>
where
    P: Provider<Ethereum> + Send + Sync,
{
    fn root(&self) -> &RootProvider {
        self.inner.root()
    }

    async fn send_raw_transaction(
        &self,
        encoded_tx: &[u8],
    ) -> TransportResult<PendingTransactionBuilder<Ethereum>> {
        // For raw transactions, we can't manage nonces since they're already encoded
        // Just pass through to the inner provider
        self.inner.send_raw_transaction(encoded_tx).await
    }

    async fn send_transaction(
        &self,
        request: TransactionRequest,
    ) -> TransportResult<PendingTransactionBuilder<Ethereum>> {
        self.send_with_nonce_management(request).await
    }
} 