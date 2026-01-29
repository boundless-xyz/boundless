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

//! File-based cache storage implementation.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

use alloy::{primitives::B256, rpc::types::Log};
use async_trait::async_trait;

use crate::db::TxMetadata;

use super::{generate_cache_key, serde::SerializableTxMetadata, CacheStorage, CacheStorageError};

#[derive(thiserror::Error, Debug)]
pub enum FileCacheStorageError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),
}

/// File-based cache storage that stores logs and metadata as JSON files.
#[derive(Clone, Debug)]
pub struct FileCacheStorage {
    base_path: PathBuf,
}

impl FileCacheStorage {
    /// Create a new file cache storage with the given base path.
    pub fn new(path: &str) -> Result<Self, FileCacheStorageError> {
        let base_path = PathBuf::from(path).join("indexer-cache");
        Ok(Self { base_path })
    }

    /// Get the full path for a logs cache file.
    fn logs_path(&self, key: &str) -> PathBuf {
        self.base_path.join(format!("{}.logs.json", key))
    }

    /// Get the full path for a metadata cache file.
    fn metadata_path(&self, key: &str) -> PathBuf {
        self.base_path.join(format!("{}.metadata.json", key))
    }

    /// Ensure the directory for a cache file exists.
    async fn ensure_dir(&self, path: &Path) -> Result<(), FileCacheStorageError> {
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        Ok(())
    }
}

#[async_trait]
impl CacheStorage for FileCacheStorage {
    async fn get_logs(
        &self,
        chain_id: u64,
        start_block: u64,
        end_block: u64,
        event_signatures: &[B256],
    ) -> Result<Option<Vec<Log>>, CacheStorageError> {
        let key = generate_cache_key(chain_id, start_block, end_block, event_signatures);
        let path = self.logs_path(&key);

        if !path.exists() {
            return Ok(None);
        }

        let data = tokio::fs::read(&path).await.map_err(FileCacheStorageError::from)?;
        let logs: Vec<Log> = serde_json::from_slice(&data).map_err(FileCacheStorageError::from)?;

        Ok(Some(logs))
    }

    async fn put_logs(
        &self,
        chain_id: u64,
        start_block: u64,
        end_block: u64,
        event_signatures: &[B256],
        logs: &[Log],
    ) -> Result<(), CacheStorageError> {
        let key = generate_cache_key(chain_id, start_block, end_block, event_signatures);
        let path = self.logs_path(&key);
        self.ensure_dir(&path).await?;

        let data = serde_json::to_vec(logs).map_err(FileCacheStorageError::from)?;
        tokio::fs::write(&path, data).await.map_err(FileCacheStorageError::from)?;

        tracing::debug!("Cached {} logs to {}", logs.len(), path.display());
        Ok(())
    }

    async fn get_tx_metadata(
        &self,
        chain_id: u64,
        start_block: u64,
        end_block: u64,
        event_signatures: &[B256],
    ) -> Result<Option<HashMap<B256, TxMetadata>>, CacheStorageError> {
        let key = generate_cache_key(chain_id, start_block, end_block, event_signatures);
        let path = self.metadata_path(&key);
        tracing::debug!("Getting tx metadata from {}", path.display());

        if !path.exists() {
            return Ok(None);
        }

        let data = tokio::fs::read(&path).await.map_err(FileCacheStorageError::from)?;
        let serializable_map: HashMap<String, SerializableTxMetadata> =
            serde_json::from_slice(&data).map_err(FileCacheStorageError::from)?;

        let metadata: HashMap<B256, TxMetadata> = serializable_map
            .into_iter()
            .map(|(k, v)| {
                let bytes = hex::decode(&k).expect("Invalid hex in cache");
                (B256::from_slice(&bytes), v.into())
            })
            .collect();

        Ok(Some(metadata))
    }

    async fn put_tx_metadata(
        &self,
        chain_id: u64,
        start_block: u64,
        end_block: u64,
        event_signatures: &[B256],
        metadata: &HashMap<B256, TxMetadata>,
    ) -> Result<(), CacheStorageError> {
        let key = generate_cache_key(chain_id, start_block, end_block, event_signatures);
        let path = self.metadata_path(&key);
        self.ensure_dir(&path).await?;

        let serializable_map: HashMap<String, SerializableTxMetadata> =
            metadata.iter().map(|(k, v)| (hex::encode(k), v.into())).collect();

        let data = serde_json::to_vec(&serializable_map).map_err(FileCacheStorageError::from)?;
        tokio::fs::write(&path, data).await.map_err(FileCacheStorageError::from)?;

        tracing::info!("Cached {} tx metadata entries to {}", metadata.len(), path.display());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::{
        network::TransactionResponse, node_bindings::Anvil, primitives::U256, providers::Provider,
        rpc::types::BlockNumberOrTag, sol_types::SolEvent,
    };
    use boundless_cli::{OrderFulfilled, OrderFulfiller};
    use boundless_market::contracts::{
        boundless_market::FulfillmentTx, IBoundlessMarket, Offer, Predicate, ProofRequest,
        RequestInput, Requirements,
    };
    use boundless_test_utils::{
        guests::{ECHO_ID, ECHO_PATH},
        market::create_test_ctx,
    };
    use broker::provers::DefaultProver;
    use std::{collections::HashSet, sync::Arc};

    async fn create_order(
        ctx: &boundless_test_utils::market::TestCtx<impl Provider + Clone>,
        order_id: u32,
        now: u64,
    ) -> (ProofRequest, alloy::primitives::Bytes) {
        use boundless_market::contracts::RequestId;

        let signer_addr = ctx.customer_signer.address();
        let req = ProofRequest::new(
            RequestId::new(signer_addr, order_id),
            Requirements::new(Predicate::prefix_match(
                ECHO_ID,
                alloy::primitives::Bytes::default(),
            )),
            format!("file://{ECHO_PATH}"),
            RequestInput::builder().build_inline().unwrap(),
            Offer {
                minPrice: U256::from(0),
                maxPrice: U256::from(1),
                rampUpStart: now - 3,
                timeout: 12,
                rampUpPeriod: 1,
                lockTimeout: 12,
                lockCollateral: U256::from(0),
            },
        );

        let client_sig = req
            .sign_request(
                &ctx.customer_signer,
                ctx.deployment.boundless_market_address,
                ctx.deployment.market_chain_id.unwrap(),
            )
            .await
            .unwrap();

        (req, client_sig.as_bytes().into())
    }

    #[tokio::test]
    #[ignore = "Generates a proof. Slow without RISC0_DEV_MODE=1"]
    async fn test_logs_and_metadata_serialization() {
        // Set up test context with anvil and market contracts
        let anvil = Anvil::new().block_time_f64(0.5).try_spawn().unwrap();
        let ctx = create_test_ctx(&anvil).await.unwrap();

        // Create a proof request
        let now = ctx
            .customer_provider
            .get_block_by_number(BlockNumberOrTag::Latest)
            .await
            .unwrap()
            .unwrap()
            .header
            .timestamp;

        let (request, client_sig) = create_order(&ctx, 1, now).await;

        // Submit, lock, and fulfill the request
        let client =
            boundless_market::Client::new(ctx.prover_market.clone(), ctx.set_verifier.clone());
        let prover =
            OrderFulfiller::initialize(Arc::new(DefaultProver::default()), &client).await.unwrap();

        ctx.customer_market.deposit(U256::from(10)).await.unwrap();
        ctx.customer_market
            .submit_request_with_signature(&request, client_sig.clone())
            .await
            .unwrap();
        ctx.prover_market.lock_request(&request, client_sig.clone()).await.unwrap();

        let (fill, root_receipt, assessor_receipt) =
            prover.fulfill(&[(request.clone(), client_sig.clone())]).await.unwrap();
        let order_fulfilled =
            OrderFulfilled::new(fill.clone(), root_receipt, assessor_receipt).unwrap();
        ctx.prover_market
            .fulfill(
                FulfillmentTx::new(order_fulfilled.fills, order_fulfilled.assessorReceipt)
                    .with_submit_root(
                        ctx.deployment.set_verifier_address,
                        order_fulfilled.root,
                        order_fulfilled.seal,
                    ),
            )
            .await
            .unwrap();

        // Get the block range
        let latest_block = ctx.customer_provider.get_block_number().await.unwrap();

        // Define event signatures we're interested in
        let event_signatures = vec![
            IBoundlessMarket::RequestSubmitted::SIGNATURE_HASH,
            IBoundlessMarket::RequestLocked::SIGNATURE_HASH,
            IBoundlessMarket::RequestFulfilled::SIGNATURE_HASH,
            IBoundlessMarket::ProofDelivered::SIGNATURE_HASH,
        ];

        // Fetch logs directly from the provider
        let filter = alloy::rpc::types::Filter::new()
            .address(*ctx.customer_market.instance().address())
            .from_block(0)
            .to_block(latest_block)
            .event_signature(event_signatures.clone());

        let original_logs = ctx.customer_provider.get_logs(&filter).await.unwrap();
        assert!(!original_logs.is_empty(), "Should have fetched some logs");

        // Collect transaction hashes and fetch transaction metadata
        let mut tx_hashes = HashSet::new();
        for log in &original_logs {
            if let Some(tx_hash) = log.transaction_hash {
                tx_hashes.insert(tx_hash);
            }
        }

        let mut original_metadata = HashMap::new();
        for tx_hash in tx_hashes {
            let tx = ctx.customer_provider.get_transaction_by_hash(tx_hash).await.unwrap().unwrap();
            let block_number = tx.block_number.unwrap();
            let block = ctx
                .customer_provider
                .get_block_by_number(BlockNumberOrTag::Number(block_number))
                .await
                .unwrap()
                .unwrap();

            let metadata = crate::db::TxMetadata {
                tx_hash,
                from: tx.from(),
                block_number,
                block_timestamp: block.header.timestamp,
                transaction_index: tx.transaction_index.unwrap_or(0),
            };
            original_metadata.insert(tx_hash, metadata);
        }

        // Create a FileCacheStorage instance
        let temp_dir = std::env::temp_dir();
        let cache_dir = temp_dir.join("test_cache");
        let cache_storage = FileCacheStorage::new(cache_dir.to_str().unwrap()).unwrap();

        // Test cache miss first
        let chain_id = anvil.chain_id();
        let missing_logs =
            cache_storage.get_logs(chain_id, 0, latest_block, &event_signatures).await.unwrap();
        assert!(missing_logs.is_none(), "Should return None for cache miss");

        // Put logs in cache
        cache_storage
            .put_logs(chain_id, 0, latest_block, &event_signatures, &original_logs)
            .await
            .unwrap();

        // Get logs from cache
        let cached_logs = cache_storage
            .get_logs(chain_id, 0, latest_block, &event_signatures)
            .await
            .unwrap()
            .expect("Should have cached logs");

        assert_eq!(original_logs.len(), cached_logs.len(), "Cached log count should match");

        // Verify each log field matches
        for (original, cached) in original_logs.iter().zip(cached_logs.iter()) {
            assert_eq!(original.address(), cached.address(), "Address should match");
            assert_eq!(original.topics(), cached.topics(), "Topics should match");
            assert_eq!(original.data(), cached.data(), "Data should match");
            assert_eq!(original.block_hash, cached.block_hash, "Block hash should match");
            assert_eq!(original.block_number, cached.block_number, "Block number should match");
            assert_eq!(
                original.transaction_hash, cached.transaction_hash,
                "Transaction hash should match"
            );
        }

        // Test metadata cache miss
        let missing_metadata = cache_storage
            .get_tx_metadata(chain_id, 0, latest_block, &event_signatures)
            .await
            .unwrap();
        assert!(missing_metadata.is_none(), "Should return None for metadata cache miss");

        // Put metadata in cache
        cache_storage
            .put_tx_metadata(chain_id, 0, latest_block, &event_signatures, &original_metadata)
            .await
            .unwrap();

        // Get metadata from cache
        let cached_metadata = cache_storage
            .get_tx_metadata(chain_id, 0, latest_block, &event_signatures)
            .await
            .unwrap()
            .expect("Should have cached metadata");

        assert_eq!(
            original_metadata.len(),
            cached_metadata.len(),
            "Cached metadata count should match"
        );

        // Verify each metadata entry matches
        for (tx_hash, original_meta) in &original_metadata {
            let cached_meta =
                cached_metadata.get(tx_hash).expect("Should have metadata for tx hash");
            assert_eq!(original_meta.tx_hash, cached_meta.tx_hash, "Tx hash should match");
            assert_eq!(original_meta.from, cached_meta.from, "From address should match");
            assert_eq!(
                original_meta.block_number, cached_meta.block_number,
                "Block number should match"
            );
            assert_eq!(
                original_meta.block_timestamp, cached_meta.block_timestamp,
                "Block timestamp should match"
            );
        }

        // Clean up temp cache directory
        let _ = std::fs::remove_dir_all(&cache_dir);
    }
}
