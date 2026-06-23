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

/// Contains RequestEvaluator implementation for risc0
#[cfg(not(target_os = "zkvm"))]
pub mod request_evaluator;

/// Contains LocalExector implementation for risc0
#[cfg(not(target_os = "zkvm"))]
pub mod local_executor;

#[cfg(not(target_os = "zkvm"))]
use std::sync::Arc;

/// Risc0 ZKVM boundless market objects
#[cfg(not(target_os = "zkvm"))]
#[derive(Clone)]
pub struct Risc0ZkvmOps {
    evaluator: Arc<request_evaluator::Risc0Evaluator>,
    local_executor: Arc<local_executor::Risc0LocalExecutor>,
}

#[cfg(not(target_os = "zkvm"))]
impl Risc0ZkvmOps {
    /// Creates a new Risc0ZkvmOps
    pub async fn new() -> Self {
        let local_executor = Arc::new(local_executor::Risc0LocalExecutor::default());

        // Create preflight cache - the LocalExecutor handles execution deduplication internally
        let preflight_cache = Arc::new(
            moka::future::Cache::builder()
                .eviction_policy(moka::policy::EvictionPolicy::lru())
                .max_capacity(32)
                .build(),
        );

        // Create a standard downloader for fetching images and inputs
        let downloader = Arc::new(crate::StandardDownloader::new().await);

        let evaluator = Arc::new(request_evaluator::Risc0Evaluator::new(
            local_executor.clone(),
            downloader,
            preflight_cache,
        ));

        Self { evaluator, local_executor }
    }
}

impl From<risc0_zkvm::sha::Digest> for crate::Digest {
    fn from(d: risc0_zkvm::sha::Digest) -> Self {
        let bytes: [u8; 32] = d.as_bytes().try_into().expect("sha::Digest is always 32 bytes");
        crate::Digest::from_bytes(bytes)
    }
}

impl From<crate::Digest> for risc0_zkvm::sha::Digest {
    fn from(d: crate::Digest) -> Self {
        risc0_zkvm::sha::Digest::try_from(d.as_bytes()).expect("crate::Digest is always 32 bytes")
    }
}

impl From<[u32; 8]> for crate::Digest {
    fn from(words: [u32; 8]) -> Self {
        crate::Digest::from(risc0_zkvm::sha::Digest::from(words))
    }
}

impl From<risc0_zkvm::Journal> for crate::Journal {
    fn from(j: risc0_zkvm::Journal) -> Self {
        crate::Journal::new(j.bytes)
    }
}

impl From<crate::Journal> for risc0_zkvm::Journal {
    fn from(j: crate::Journal) -> Self {
        risc0_zkvm::Journal::new(j.bytes)
    }
}

#[cfg(not(target_os = "zkvm"))]
impl crate::request_builder::ZkvmOps for Risc0ZkvmOps {
    fn executor(&self) -> Arc<dyn crate::request_builder::LocalExecutor + Sync + Send> {
        self.local_executor.clone()
    }

    fn evaluator(&self) -> Arc<dyn crate::prover_utils::RequestEvaluator + Sync + Send> {
        self.evaluator.clone()
    }

    fn compute_image_id(&self, program: &[u8]) -> anyhow::Result<crate::Digest> {
        Ok(risc0_zkvm::compute_image_id(program)?.into())
    }
}
