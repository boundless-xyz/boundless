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

//! Chain provider utilities for consistent provider creation and configuration

use alloy::{
    network::EthereumWallet,
    providers::{Provider, ProviderBuilder},
    signers::local::PrivateKeySigner,
};
use anyhow::{Context, Result};
use std::marker::PhantomData;
use std::sync::Arc;

/// Standard provider builder for CLI commands
pub struct ChainProvider<P = ()> {
    rpc_url: String,
    signer: Option<PrivateKeySigner>,
    _phantom: PhantomData<P>,
}

impl ChainProvider {
    /// Create a new provider builder with RPC URL
    pub fn new(rpc_url: impl Into<String>) -> Self {
        Self {
            rpc_url: rpc_url.into(),
            signer: None,
            _phantom: PhantomData,
        }
    }

    /// Add a signer to the provider
    pub fn with_signer(mut self, signer: PrivateKeySigner) -> Self {
        self.signer = Some(signer);
        self
    }

    /// Build and connect the provider
    pub async fn connect(self) -> Result<Arc<dyn Provider<alloy::network::Ethereum> + Send + Sync>> {
        let rpc_url = self.rpc_url.clone();

        if let Some(signer) = self.signer {
            let provider = ProviderBuilder::new()
                .wallet(EthereumWallet::from(signer))
                .connect(&rpc_url)
                .await
                .with_context(|| format!("Failed to connect provider to {}", rpc_url))?;
            Ok(Arc::new(provider))
        } else {
            let provider = ProviderBuilder::new()
                .connect(&rpc_url)
                .await
                .with_context(|| format!("Failed to connect provider to {}", rpc_url))?;
            Ok(Arc::new(provider))
        }
    }

    /// Build provider and get chain ID
    pub async fn connect_with_chain_id(self) -> Result<(Arc<dyn Provider<alloy::network::Ethereum> + Send + Sync>, u64)> {
        let provider = self.connect().await?;
        let chain_id = provider
            .get_chain_id()
            .await
            .context("Failed to get chain ID")?;
        Ok((provider, chain_id))
    }

    /// Build provider with chain ID and deployment resolution
    pub async fn connect_with_deployment<D>(self) -> Result<(Arc<dyn Provider<alloy::network::Ethereum> + Send + Sync>, u64, Option<D>)>
    where
        D: DeploymentFromChainId,
    {
        let (provider, chain_id) = self.connect_with_chain_id().await?;
        let deployment = D::from_chain_id(chain_id);
        Ok((provider, chain_id, deployment))
    }
}

/// Trait for types that can be resolved from a chain ID
pub trait DeploymentFromChainId: Sized {
    /// Try to resolve a deployment from a chain ID
    fn from_chain_id(chain_id: u64) -> Option<Self>;
}

// Implementation for boundless_market::Deployment
impl DeploymentFromChainId for boundless_market::Deployment {
    fn from_chain_id(chain_id: u64) -> Option<Self> {
        boundless_market::Deployment::from_chain_id(chain_id)
    }
}

// Implementation for boundless_zkc::deployments::Deployment
impl DeploymentFromChainId for boundless_zkc::deployments::Deployment {
    fn from_chain_id(chain_id: u64) -> Option<Self> {
        boundless_zkc::deployments::Deployment::from_chain_id(chain_id)
    }
}

// Implementation for boundless_povw::deployments::Deployment
impl DeploymentFromChainId for boundless_povw::deployments::Deployment {
    fn from_chain_id(chain_id: u64) -> Option<Self> {
        boundless_povw::deployments::Deployment::from_chain_id(chain_id)
    }
}

/// Helper to create a provider from rewards config
pub async fn provider_from_rewards_config(
    rpc_url: &str,
    signer: Option<PrivateKeySigner>,
) -> Result<Arc<dyn Provider<alloy::network::Ethereum> + Send + Sync>> {
    let builder = ChainProvider::new(rpc_url);
    let builder = if let Some(s) = signer {
        builder.with_signer(s)
    } else {
        builder
    };
    builder.connect().await
}

/// Helper to create a provider for a specific chain with deployment
pub async fn provider_with_deployment<D>(
    rpc_url: &str,
    signer: Option<PrivateKeySigner>,
) -> Result<(Arc<dyn Provider<alloy::network::Ethereum> + Send + Sync>, u64, Option<D>)>
where
    D: DeploymentFromChainId,
{
    let builder = ChainProvider::new(rpc_url);
    let builder = if let Some(s) = signer {
        builder.with_signer(s)
    } else {
        builder
    };
    builder.connect_with_deployment().await
}

/// Quick helper to get chain ID from RPC URL
pub async fn get_chain_id(rpc_url: &str) -> Result<u64> {
    let provider = ProviderBuilder::new()
        .connect(rpc_url)
        .await
        .with_context(|| format!("Failed to connect to {}", rpc_url))?;

    provider.get_chain_id().await.context("Failed to get chain ID")
}

/// Batch provider operations for efficiency
pub struct BatchProviderOps<P: Provider> {
    provider: P,
}

impl<P: Provider> BatchProviderOps<P> {
    /// Create a new batch provider operations wrapper
    pub fn new(provider: P) -> Self {
        Self { provider }
    }

    /// Get multiple balances in parallel
    pub async fn get_balances(
        &self,
        addresses: Vec<alloy::primitives::Address>,
    ) -> Result<Vec<alloy::primitives::U256>> {
        use futures::future::try_join_all;

        let futures: Vec<_> = addresses
            .into_iter()
            .map(|addr| {
                let provider = &self.provider;
                async move {
                    provider.get_balance(addr).await
                }
            })
            .collect();

        try_join_all(futures)
            .await
            .context("Failed to get balances")
    }

    /// Get block number and timestamp together
    pub async fn get_block_info(&self) -> Result<(u64, u64)> {
        let block_number = self
            .provider
            .get_block_number()
            .await
            .context("Failed to get block number")?;

        let block = self
            .provider
            .get_block_by_number(block_number.into())
            .await
            .context("Failed to get block")?
            .context("Block not found")?;

        Ok((block_number, block.header.inner.timestamp))
    }
}

/// Standard provider configuration for different contexts
pub mod configs {
    use super::*;

    /// Create a read-only provider (no signer needed)
    pub async fn read_only_provider(rpc_url: &str) -> Result<Arc<dyn Provider<alloy::network::Ethereum> + Send + Sync>> {
        ChainProvider::new(rpc_url).connect().await
    }

    /// Create a provider for transaction sending
    pub async fn tx_provider(
        rpc_url: &str,
        signer: PrivateKeySigner,
    ) -> Result<Arc<dyn Provider<alloy::network::Ethereum> + Send + Sync>> {
        ChainProvider::new(rpc_url)
            .with_signer(signer)
            .connect()
            .await
    }

    /// Create a provider with retry logic for unreliable networks
    pub async fn resilient_provider(
        rpc_url: &str,
        max_retries: u32,
    ) -> Result<Arc<dyn Provider<alloy::network::Ethereum> + Send + Sync>> {
        let mut last_error = None;

        for attempt in 1..=max_retries {
            match ChainProvider::new(rpc_url).connect().await {
                Ok(provider) => return Ok(provider),
                Err(e) => {
                    tracing::warn!("Provider connection attempt {} failed: {}", attempt, e);
                    last_error = Some(e);
                    if attempt < max_retries {
                        tokio::time::sleep(std::time::Duration::from_secs(attempt as u64)).await;
                    }
                }
            }
        }

        Err(last_error.unwrap_or_else(|| anyhow::anyhow!("Failed to connect after {} attempts", max_retries)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_chain_provider_creation() {
        // Test that provider can be created (will fail without actual RPC)
        let result = ChainProvider::new("http://localhost:8545").connect().await;
        assert!(result.is_err()); // Expected to fail without running node
    }

    #[test]
    fn test_provider_builder_pattern() {
        let signer = PrivateKeySigner::random();
        let _provider = ChainProvider::new("http://localhost:8545")
            .with_signer(signer);
        // Just testing that the builder pattern compiles
    }
}