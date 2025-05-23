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
    network::{Ethereum, EthereumWallet, NetworkWallet, TransactionBuilder},
    primitives::Address,
    providers::{
        fillers::{FillProvider, TxFiller},
        PendingTransactionBuilder, Provider, RootProvider, SendableTx, WalletProvider,
    },
    rpc::types::TransactionRequest,
    transports::{RpcError, TransportResult},
};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{Mutex, Semaphore};

/// A provider that manages nonces per account using semaphores.
///
/// This provider exists to avoid nonce collisions when submitting transactions concurrently.
/// It does so by holding a semaphore permit between fetching the pending nonce of the signer until
/// the transaction is sent.
#[derive(Clone, Debug)]
pub struct NonceProvider<F, P>
where
    F: TxFiller<Ethereum>,
    P: Provider<Ethereum> + Send + Sync,
{
    inner: Arc<FillProvider<F, P, Ethereum>>,
    wallet: EthereumWallet,
    account_semaphores: Arc<Mutex<HashMap<Address, Arc<Semaphore>>>>,
}

impl<F, P> NonceProvider<F, P>
where
    F: TxFiller<Ethereum>,
    P: Provider<Ethereum> + Send + Sync,
{
    pub fn new(inner: FillProvider<F, P, Ethereum>, wallet: EthereumWallet) -> Self {
        Self {
            inner: Arc::new(inner),
            wallet,
            account_semaphores: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Get or create a semaphore for the given account address.
    async fn get_account_semaphore(&self, address: Address) -> Arc<Semaphore> {
        let mut semaphores = self.account_semaphores.lock().await;
        semaphores.entry(address).or_insert_with(|| Arc::new(Semaphore::new(1))).clone()
    }
}

#[async_trait::async_trait]
impl<F, P> Provider<Ethereum> for NonceProvider<F, P>
where
    F: TxFiller<Ethereum>,
    P: Provider<Ethereum> + Send + Sync + std::fmt::Debug,
{
    fn root(&self) -> &RootProvider<Ethereum> {
        self.inner.root()
    }

    async fn send_transaction(
        &self,
        mut request: TransactionRequest,
    ) -> TransportResult<PendingTransactionBuilder<Ethereum>> {
        let from_address = if let Some(from) = request.from {
            from
        } else {
            <EthereumWallet as NetworkWallet<Ethereum>>::default_signer_address(&self.wallet)
        };
        request.set_from(from_address);

        // Get semaphore for this account and acquire permit
        let semaphore = self.get_account_semaphore(from_address).await;
        let _permit = semaphore.acquire().await.unwrap();

        // Fetch the pending nonce if not already set
        if request.nonce.is_none() {
            let pending_nonce = self.inner.get_transaction_count(from_address).pending().await?;
            request.nonce = Some(pending_nonce);
            tracing::trace!(
                "NonceProvider::send_with_nonce_management - set nonce {} for address: {}",
                pending_nonce,
                from_address
            );
        }

        let tx = self.inner.fill(request).await.unwrap();

        let builder = match tx {
            SendableTx::Builder(builder) => builder,
            _ => {
                panic!("should not be called test");
                // return Ok(tx);
            }
        };

        let envelope = builder.build(&self.wallet).await.map_err(RpcError::local_usage)?;

        self.inner.send_transaction_internal(SendableTx::Envelope(envelope)).await
    }
}

impl<F, P> WalletProvider<Ethereum> for NonceProvider<F, P>
where
    F: TxFiller<Ethereum>,
    P: Provider<Ethereum> + Send + Sync + std::fmt::Debug,
{
    type Wallet = EthereumWallet;

    fn wallet(&self) -> &Self::Wallet {
        &self.wallet
    }

    fn wallet_mut(&mut self) -> &mut Self::Wallet {
        &mut self.wallet
    }

    fn default_signer_address(&self) -> Address {
        <EthereumWallet as NetworkWallet<Ethereum>>::default_signer_address(&self.wallet)
    }
}
