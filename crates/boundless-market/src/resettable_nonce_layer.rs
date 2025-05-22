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

use std::sync::Arc;

use alloy::{
    network::{Ethereum, Network},
    primitives::Address,
    providers::{
        fillers::NonceManager, PendingTransactionBuilder, Provider, ProviderLayer, RootProvider,
    },
    transports::TransportResult,
};
use async_trait::async_trait;
use dashmap::DashMap;
use futures::lock::Mutex;

/// Resettable cached nonce manager
///
/// This [`NonceManager`] implementation will fetch the transaction count for any new account it
/// sees, store it locally and increment the locally stored nonce as transactions are sent via
/// [`Provider::send_transaction`].
#[derive(Clone, Debug, Default)]
pub struct ResettableNonceManager {
    nonces: Arc<DashMap<Address, Arc<Mutex<u64>>>>,
}

#[cfg_attr(target_family = "wasm", async_trait(?Send))]
#[cfg_attr(not(target_family = "wasm"), async_trait)]
impl NonceManager for ResettableNonceManager {
    async fn get_next_nonce<P, N>(&self, provider: &P, address: Address) -> TransportResult<u64>
    where
        P: Provider<N>,
        N: Network,
    {
        // Use `u64::MAX` as a sentinel value to indicate that the nonce has not been fetched yet.
        const NONE: u64 = u64::MAX;

        // Locks dashmap internally for a short duration to clone the `Arc`.
        // We also don't want to hold the dashmap lock through the await point below.
        let nonce = {
            let rm = self.nonces.entry(address).or_insert_with(|| Arc::new(Mutex::new(NONE)));
            Arc::clone(rm.value())
        };

        let mut nonce = nonce.lock().await;
        let new_nonce = if *nonce == NONE {
            // Initialize the nonce if we haven't seen this account before.
            tracing::trace!(%address, "fetching nonce");
            provider.get_transaction_count(address).await?
        } else {
            tracing::trace!(%address, current_nonce = *nonce, "incrementing nonce");
            *nonce + 1
        };
        *nonce = new_nonce;
        Ok(new_nonce)
    }
}

impl ResettableNonceManager {
    /// Clears the nonce cache for the given address.
    /// The next `get_next_nonce` call will fetch it from the Ethereum node automatically.
    pub fn reset_nonce(&self, address: Address) {
        self.nonces.remove(&address);
        tracing::info!(%address, "Nonce cache cleared");
    }
}

#[derive(Debug, Clone, Default)]
/// A [`ProviderLayer`] that resets the nonce cache for a given address
/// when a "nonce too low" error is detected.
pub struct NonceResetLayer {
    address: Address,
    nonce_manager: ResettableNonceManager,
}

impl NonceResetLayer {
    /// Creates a new NonceResetLayer with the given configuration.
    pub fn new(address: Address, nonce_manager: ResettableNonceManager) -> Self {
        Self { address, nonce_manager }
    }

    /// Returns the nonce manager used by this layer.
    pub fn nonce_manager(&self) -> &ResettableNonceManager {
        &self.nonce_manager
    }
}

impl<P> ProviderLayer<P> for NonceResetLayer
where
    P: Provider,
{
    type Provider = NonceResetProvider<P>;

    fn layer(&self, inner: P) -> Self::Provider {
        NonceResetProvider {
            inner,
            address: self.address,
            nonce_manager: self.nonce_manager.clone(),
        }
    }
}

#[derive(Clone, Debug)]
/// A [`ProviderLayer`] that resets the nonce cache for a given address
/// when a "nonce too low" error is detected.
pub struct NonceResetProvider<P> {
    inner: P,
    address: Address,
    nonce_manager: ResettableNonceManager,
}

#[async_trait::async_trait]
impl<P> Provider for NonceResetProvider<P>
where
    P: Provider,
{
    fn root(&self) -> &RootProvider {
        self.inner.root()
    }

    async fn send_raw_transaction(
        &self,
        encoded_tx: &[u8],
    ) -> TransportResult<PendingTransactionBuilder<Ethereum>> {
        let res = self.inner.send_raw_transaction(encoded_tx).await;

        if let Err(err) = &res {
            if err.to_string().to_lowercase().contains("nonce too low") {
                tracing::warn!("Nonce error detected, resetting nonce cache");
                self.nonce_manager.reset_nonce(self.address);
            }
        }

        res
    }
}
