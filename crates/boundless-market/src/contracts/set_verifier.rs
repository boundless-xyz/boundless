// Copyright (c) 2024 RISC Zero, Inc.
//
// All rights reserved.

use std::time::Duration;

use super::{
    IRiscZeroSetVerifier::{self, IRiscZeroSetVerifierErrors, IRiscZeroSetVerifierInstance},
    TXN_CONFIRM_TIMEOUT,
};
use crate::contracts::set_verifier::IRiscZeroVerifier::IRiscZeroVerifierInstance;
use alloy::{
    network::Ethereum,
    primitives::{Address, Bytes, B256},
    transports::Transport,
};
use alloy_provider;
use anyhow::{Context, Result};

//TODO: drop this once risc0-ethereum is updated
alloy::sol!(
    #![sol(rpc, all_derives)]
    "../../lib/risc0-ethereum/contracts/src/IRiscZeroVerifier.sol"
);

#[derive(Clone)]
pub struct SetVerifierService<T, P> {
    instance: IRiscZeroSetVerifierInstance<T, P, Ethereum>,
    verifier_instance: IRiscZeroVerifierInstance<T, P, Ethereum>,
    caller: Address,
    tx_timeout: Duration,
}

impl<T, P> SetVerifierService<T, P>
where
    T: Transport + Clone,
    P: alloy_provider::Provider<T, Ethereum> + 'static + Clone,
{
    pub fn new(address: Address, provider: P, caller: Address) -> Self {
        let instance = IRiscZeroSetVerifier::new(address, provider.clone());
        let verifier_instance = IRiscZeroVerifier::new(address, provider);

        Self { instance, verifier_instance, caller, tx_timeout: TXN_CONFIRM_TIMEOUT }
    }

    pub fn instance(&self) -> &IRiscZeroSetVerifierInstance<T, P, Ethereum> {
        &self.instance
    }

    pub fn with_timeout(self, tx_timeout: Duration) -> Self {
        Self { tx_timeout, ..self }
    }

    pub async fn contains_root(&self, root: B256) -> Result<bool> {
        tracing::debug!("Calling containsRoot({:?})", root);
        let call = self.instance.containsRoot(root);

        Ok(call.call().await.context("call failed")?._0)
    }

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

    pub async fn verify(&self, seal: Bytes, image_id: B256, journal_digest: B256) -> Result<()> {
        tracing::debug!("Calling verify({:?},{:?},{:?})", seal, image_id, journal_digest);
        self.verifier_instance
            .verify(seal, image_id, journal_digest)
            .call()
            .await
            .map_err(|_| anyhow::anyhow!("Verification failed"))?;

        Ok(())
    }

    pub async fn image_info(&self) -> Result<(B256, String)> {
        tracing::debug!("Calling imageInfo()");
        let (image_id, image_url) =
            self.instance.imageInfo().call().await.context("call failed")?.into();

        Ok((image_id, image_url))
    }
}
