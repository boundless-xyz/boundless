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

use std::sync::Arc;

use async_trait::async_trait;
use moka::future::Cache;
use moka::policy::EvictionPolicy;
use risc0_zkvm::sha::Digest;
use risc0_zkvm::{default_executor, ExecutorEnv, SessionInfo};
use sha2::{Digest as Sha2Digest, Sha256};

use super::prover::{ExecutorResp, ProofResult, Prover, ProverError};

/// A local executor that runs zkVM programs in-memory for preflight operations.
///
/// This executor is designed for requestor-side pricing checks. It supports:
/// - Uploading and storing images/inputs in memory with LRU eviction
/// - Running preflight execution to count cycles
/// - Retrieving journals from preflight executions
/// - Content-addressed deduplication using input hashes
///
/// Caches are bounded to prevent unbounded memory growth:
/// - Inputs cache: up to 64 entries (LRU eviction)
/// - Images cache: up to 32 entries (LRU eviction)
/// - Executions cache: up to 64 entries (LRU eviction)
///
/// It does **not** support actual proving operations (STARK/SNARK/Groth16).
/// Those operations will return errors.
#[derive(Clone)]
pub struct LocalExecutor {
    state: Arc<ExecutorState>,
}

struct ExecutorState {
    inputs: Cache<String, Vec<u8>>,
    images: Cache<String, Vec<u8>>,
    executions: Cache<String, ExecutionData>,
}

#[derive(Debug, Clone, Default)]
struct ExecutionData {
    stats: Option<ExecutorResp>,
    preflight_journal: Option<Vec<u8>>,
}

impl Default for LocalExecutor {
    fn default() -> Self {
        Self {
            state: Arc::new(ExecutorState {
                inputs: Cache::builder()
                    .eviction_policy(EvictionPolicy::lru())
                    .max_capacity(32)
                    .build(),
                images: Cache::builder()
                    .eviction_policy(EvictionPolicy::lru())
                    .max_capacity(32)
                    .build(),
                executions: Cache::builder()
                    .eviction_policy(EvictionPolicy::lru())
                    .max_capacity(64)
                    .build(),
            }),
        }
    }
}

impl LocalExecutor {
    /// Hash input bytes to create a content-addressed ID.
    pub fn hash_input(input: &[u8]) -> String {
        let hash = Sha256::digest(input);
        hex::encode(hash)
    }

    /// Create execution ID from image_id and input_hash for deduplication.
    fn make_exec_id(image_id: &str, input_hash: &str) -> String {
        format!("{}_{}", image_id, input_hash)
    }

    /// Manually insert execution data directly into the cache.
    pub async fn insert_execution_data(
        &self,
        image_id: &str,
        input: &[u8],
        total_cycles: u64,
        journal: Vec<u8>,
    ) {
        let input_hash = Self::hash_input(input);
        let exec_id = Self::make_exec_id(image_id, &input_hash);

        let data = ExecutionData {
            stats: Some(ExecutorResp {
                total_cycles,
                user_cycles: total_cycles,
                segments: 1,
                assumption_count: 0,
            }),
            preflight_journal: Some(journal),
        };

        tracing::debug!("Pre-filling execution cache for {} with {} cycles", exec_id, total_cycles);

        self.state.executions.insert(exec_id, data).await;
    }

    /// Execute a program with the given input, deduplicating by content.
    ///
    /// Returns the execution stats and journal. If the same image_id + input
    /// combination was already executed, returns the cached result.
    pub async fn execute_program(
        &self,
        image_id: &str,
        program: &[u8],
        input: &[u8],
    ) -> Result<(ExecutorResp, Vec<u8>), ProverError> {
        // Store image and input, get the content-addressed input ID
        self.upload_image(image_id, program.to_vec()).await?;
        let input_id = self.upload_input(input.to_vec()).await?;

        // Delegate to preflight (handles caching and execution)
        let result = self.preflight(image_id, &input_id, vec![], None, "").await?;

        // Fetch the journal
        let journal = self
            .get_preflight_journal(&result.id)
            .await?
            .ok_or_else(|| ProverError::NotFound("journal".to_string()))?;

        Ok((result.stats, journal))
    }

    async fn run_execution(
        program: Vec<u8>,
        input: Vec<u8>,
        executor_limit: Option<u64>,
    ) -> Result<SessionInfo, ProverError> {
        tokio::task::spawn_blocking(move || {
            let mut env_builder = ExecutorEnv::builder();
            env_builder.session_limit(executor_limit);
            env_builder.write_slice(&input);
            let env = env_builder.build()?;
            default_executor().execute(env, &program)
        })
        .await
        .map_err(|e| ProverError::ProverInternalError(format!("execution task panicked: {e}")))?
        .map_err(|e| ProverError::ProvingFailed(e.to_string()))
    }

    async fn get_input(&self, id: &str) -> Option<Vec<u8>> {
        self.state.inputs.get(id).await
    }

    async fn get_image(&self, id: &str) -> Option<Vec<u8>> {
        self.state.images.get(id).await
    }
}

#[async_trait]
impl Prover for LocalExecutor {
    async fn has_image(&self, image_id: &str) -> Result<bool, ProverError> {
        Ok(self.state.images.contains_key(image_id))
    }

    async fn upload_input(&self, input: Vec<u8>) -> Result<String, ProverError> {
        let input_id = Self::hash_input(&input);
        self.state.inputs.insert(input_id.clone(), input).await;
        Ok(input_id)
    }

    async fn upload_image(&self, image_id: &str, image: Vec<u8>) -> Result<(), ProverError> {
        self.state.images.insert(image_id.to_string(), image).await;
        Ok(())
    }

    async fn preflight(
        &self,
        image_id: &str,
        input_id: &str,
        _assumptions: Vec<String>,
        executor_limit: Option<u64>,
        _order_id: &str,
    ) -> Result<ProofResult, ProverError> {
        let exec_id = Self::make_exec_id(image_id, input_id);

        // Check if already executed (cache hit)
        if let Some(data) = self.state.executions.get(&exec_id).await {
            if let Some(stats) = data.stats.as_ref() {
                tracing::debug!("Preflight cache hit for {}", exec_id);
                return Ok(ProofResult { id: exec_id, stats: stats.clone(), ..Default::default() });
            }
        }

        tracing::trace!("Preflight cache miss for {}, running...", exec_id);

        let image = self
            .get_image(image_id)
            .await
            .ok_or_else(|| ProverError::NotFound(format!("image {image_id}")))?;
        let input = self
            .get_input(input_id)
            .await
            .ok_or_else(|| ProverError::NotFound(format!("input {input_id}")))?;

        // Note: assumptions are not supported in the local executor for preflight
        let execute_result = Self::run_execution(image, input, executor_limit).await;

        match execute_result {
            Ok(info) => {
                let stats = ExecutorResp {
                    segments: info.segments.len() as u64,
                    user_cycles: info.cycles(),
                    total_cycles: info.cycles(),
                    ..Default::default()
                };

                self.state
                    .executions
                    .insert(
                        exec_id.clone(),
                        ExecutionData {
                            stats: Some(stats.clone()),
                            preflight_journal: Some(info.journal.bytes),
                        },
                    )
                    .await;

                Ok(ProofResult { id: exec_id, stats, ..Default::default() })
            }
            Err(err) => {
                self.state.executions.insert(exec_id, ExecutionData::default()).await;
                Err(err)
            }
        }
    }

    #[allow(unused)]
    async fn prove_stark(
        &self,
        _image_id: &str,
        _input_id: &str,
        _assumptions: Vec<String>,
    ) -> Result<String, ProverError> {
        Err(ProverError::ProverInternalError(
            "LocalExecutor does not support STARK proving. Use for preflight only.".to_string(),
        ))
    }

    #[allow(unused)]
    async fn wait_for_stark(&self, _proof_id: &str) -> Result<ProofResult, ProverError> {
        Err(ProverError::ProverInternalError(
            "LocalExecutor does not support STARK proving. Use for preflight only.".to_string(),
        ))
    }

    #[allow(unused)]
    async fn cancel_stark(&self, _proof_id: &str) -> Result<(), ProverError> {
        Err(ProverError::ProverInternalError(
            "LocalExecutor does not support STARK proving. Use for preflight only.".to_string(),
        ))
    }

    #[allow(unused)]
    async fn get_receipt(
        &self,
        _proof_id: &str,
    ) -> Result<Option<super::prover::ProverReceipt>, ProverError> {
        Err(ProverError::ProverInternalError(
            "LocalExecutor does not support receipts. Use for preflight only.".to_string(),
        ))
    }

    async fn get_preflight_journal(&self, proof_id: &str) -> Result<Option<Vec<u8>>, ProverError> {
        let execution = self
            .state
            .executions
            .get(proof_id)
            .await
            .ok_or_else(|| ProverError::NotFound(format!("execution {proof_id}")))?;
        Ok(execution.preflight_journal.clone())
    }

    #[allow(unused)]
    async fn get_journal(&self, proof_id: &str) -> Result<Option<Vec<u8>>, ProverError> {
        // For local executor, journal is only available from preflight
        self.get_preflight_journal(proof_id).await
    }

    #[allow(unused)]
    async fn compress(&self, _proof_id: &str) -> Result<String, ProverError> {
        Err(ProverError::ProverInternalError(
            "LocalExecutor does not support compression. Use for preflight only.".to_string(),
        ))
    }

    #[allow(unused)]
    async fn get_compressed_receipt(
        &self,
        _proof_id: &str,
    ) -> Result<Option<Vec<u8>>, ProverError> {
        Err(ProverError::ProverInternalError(
            "LocalExecutor does not support compression. Use for preflight only.".to_string(),
        ))
    }

    #[allow(unused)]
    async fn compress_blake3_groth16(&self, _proof_id: &str) -> Result<String, ProverError> {
        Err(ProverError::ProverInternalError(
            "LocalExecutor does not support Blake3 Groth16 compression. Use for preflight only."
                .to_string(),
        ))
    }

    #[allow(unused)]
    async fn get_blake3_groth16_receipt(
        &self,
        _proof_id: &str,
    ) -> Result<Option<Vec<u8>>, ProverError> {
        Err(ProverError::ProverInternalError(
            "LocalExecutor does not support Blake3 Groth16 receipts. Use for preflight only."
                .to_string(),
        ))
    }

    async fn compute_image_id(&self, elf: &[u8]) -> Result<Digest, ProverError> {
        Ok(risc0_zkvm::compute_image_id(elf)?)
    }

    async fn compute_claim_digest(
        &self,
        image_id: Digest,
        journal: &[u8],
    ) -> Result<Digest, ProverError> {
        use risc0_zkvm::{sha::Digestible, ReceiptClaim};
        Ok(ReceiptClaim::ok(image_id, journal.to_vec()).digest())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use boundless_test_utils::guests::{ECHO_ELF, ECHO_ID};
    use risc0_zkvm::sha::Digest;

    #[tokio::test]
    async fn test_upload_input_and_image() {
        let executor = LocalExecutor::default();

        // Test input upload
        let input_data = b"Hello, World!".to_vec();
        let input_id = executor.upload_input(input_data.clone()).await.unwrap();

        // Test image upload
        let image_id = Digest::from(ECHO_ID).to_string();
        executor.upload_image(&image_id, ECHO_ELF.to_vec()).await.unwrap();

        // Verify input was stored
        let stored_input = executor.get_input(&input_id).await.unwrap();
        assert_eq!(stored_input, input_data);

        // Verify image was stored
        let stored_image = executor.get_image(&image_id).await.unwrap();
        assert_eq!(stored_image.as_slice(), ECHO_ELF);
    }

    #[tokio::test]
    async fn test_preflight() {
        let executor = LocalExecutor::default();

        // Upload test data
        let input_data = b"Hello, World!".to_vec();
        let input_id = executor.upload_input(input_data.clone()).await.unwrap();
        let image_id = Digest::from(ECHO_ID).to_string();
        executor.upload_image(&image_id, ECHO_ELF.to_vec()).await.unwrap();

        // Run preflight
        let result =
            executor.preflight(&image_id, &input_id, vec![], None, "test_order_id").await.unwrap();
        assert!(!result.id.is_empty());
        assert!(result.stats.segments > 0 && result.stats.user_cycles > 0);

        // Fetch the journal
        let journal = executor.get_preflight_journal(&result.id).await.unwrap().unwrap();
        assert_eq!(journal, input_data);
    }

    #[tokio::test]
    async fn test_execute_program_deduplication() {
        let executor = LocalExecutor::default();
        let image_id = Digest::from(ECHO_ID).to_string();
        let input = b"Hello, World!".to_vec();

        // First execution
        let (stats1, journal1) =
            executor.execute_program(&image_id, ECHO_ELF, &input).await.unwrap();
        assert!(stats1.user_cycles > 0);
        assert_eq!(journal1, input);

        // Second execution with same input should return cached result
        let (stats2, journal2) =
            executor.execute_program(&image_id, ECHO_ELF, &input).await.unwrap();
        assert_eq!(stats1.user_cycles, stats2.user_cycles);
        assert_eq!(journal1, journal2);
    }

    #[tokio::test]
    async fn test_prove_stark_returns_error() {
        let executor = LocalExecutor::default();
        let result = executor.prove_stark("image", "input", vec![]).await;
        assert!(matches!(result, Err(ProverError::ProverInternalError(_))));
    }

    #[tokio::test]
    async fn test_insert_execution_data() {
        let executor = LocalExecutor::default();
        let image_id = Digest::from(ECHO_ID).to_string();
        let input = b"Hello, World!".to_vec();
        let cycles = 12345678u64;
        let journal = b"test journal".to_vec();

        // Pre-fill cache
        executor.insert_execution_data(&image_id, &input, cycles, journal.clone()).await;

        // Upload image and input (required for preflight to find them)
        executor.upload_image(&image_id, ECHO_ELF.to_vec()).await.unwrap();
        let input_id = executor.upload_input(input).await.unwrap();

        // Preflight should return cached data without execution
        let result = executor.preflight(&image_id, &input_id, vec![], None, "test").await.unwrap();

        assert_eq!(result.stats.total_cycles, cycles);

        // Journal should also be cached
        let cached_journal = executor.get_preflight_journal(&result.id).await.unwrap();
        assert_eq!(cached_journal, Some(journal));
    }
}
