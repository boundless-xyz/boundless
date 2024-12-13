// Copyright 2024 RISC Zero, Inc.
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

use std::time::Duration;

use super::{
    IRiscZeroSetVerifier::{self, IRiscZeroSetVerifierErrors, IRiscZeroSetVerifierInstance},
    TXN_CONFIRM_TIMEOUT,
};
use alloy::{
    network::Ethereum,
    primitives::{Address, Bytes, B256},
    providers::Provider,
    transports::Transport,
};
use anyhow::{Context, Result};
use risc0_ethereum_contracts::IRiscZeroVerifier;

/// SetVerifierService provides a high-level interface to the SetVerifier contract.
#[derive(Clone)]
pub struct SetVerifierService<T, P> {
    instance: IRiscZeroSetVerifierInstance<T, P, Ethereum>,
    caller: Address,
    tx_timeout: Duration,
}

impl<T, P> SetVerifierService<T, P>
where
    T: Transport + Clone,
    P: Provider<T, Ethereum> + 'static + Clone,
{
    /// Creates a new SetVerifierService.
    pub fn new(address: Address, provider: P, caller: Address) -> Self {
        let instance = IRiscZeroSetVerifier::new(address, provider);

        Self { instance, caller, tx_timeout: TXN_CONFIRM_TIMEOUT }
    }

    /// Returns the underlying IRiscZeroSetVerifierInstance.
    pub fn instance(&self) -> &IRiscZeroSetVerifierInstance<T, P, Ethereum> {
        &self.instance
    }

    /// Sets the timeout for transaction confirmation.
    pub fn with_timeout(self, tx_timeout: Duration) -> Self {
        Self { tx_timeout, ..self }
    }

    /// Returns whether `root` has been submitted.
    pub async fn contains_root(&self, root: B256) -> Result<bool> {
        tracing::debug!("Calling containsRoot({:?})", root);
        let call = self.instance.containsRoot(root);

        Ok(call.call().await.context("call failed")?._0)
    }

    /// Publishes a new root of a proof aggregation.
    pub async fn submit_merkle_root(&self, root: B256, seal: Bytes) -> Result<()> {
        tracing::debug!("Calling submitMerkleRoot({:?},{:?})", root, seal);
        let call = self.instance.submitMerkleRoot(root, seal).from(self.caller);
        let pending_tx = call.send().await.map_err(IRiscZeroSetVerifierErrors::decode_error)?;
        tracing::debug!("Broadcasting tx {}", pending_tx.tx_hash());
        let tx_hash = pending_tx
            .with_timeout(Some(self.tx_timeout))
            .watch()
            .await
            .context("failed to confirm tx")?;

        tracing::info!("Submitted Merkle root {}: {}", root, tx_hash);

        Ok(())
    }

    /// Verifies a RISC Zero proof of execution against an image ID and journal digest.
    ///
    /// # Arguments
    /// * `seal` - The encoded cryptographic proof (SNARK)
    /// * `image_id` - Guest program identifier
    /// * `journal_digest` - SHA-256 digest of the journal bytes
    ///
    /// # Returns
    /// * `Ok(())` if the proof is valid
    /// * `Err(_)` if verification fails
    pub async fn verify(&self, seal: Bytes, image_id: B256, journal_digest: B256) -> Result<()> {
        tracing::debug!("Calling verify({:?},{:?},{:?})", seal, image_id, journal_digest);
        let verifier =
            IRiscZeroVerifier::new(*self.instance().address(), self.instance().provider().clone());
        verifier
            .verify(seal, image_id, journal_digest)
            .call()
            .await
            .map_err(|_| anyhow::anyhow!("Verification failed"))?;

        Ok(())
    }

    /// Returns the set builder image ID and its url.
    pub async fn image_info(&self) -> Result<(B256, String)> {
        tracing::debug!("Calling imageInfo()");
        let (image_id, image_url) =
            self.instance.imageInfo().call().await.context("call failed")?.into();

        Ok((image_id, image_url))
    }
}
