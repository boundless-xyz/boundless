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
pub mod request_evaluator;

/// Contains LocalExector implementation for risc0
pub mod local_executor;

use std::sync::Arc;

/// Risc0 ZKVM boundless market objects
#[derive(Clone)]
pub struct Risc0ZkvmOps {
    evaluator: Arc<request_evaluator::Risc0Evaluator>,
    local_executor: Arc<local_executor::Risc0LocalExecutor>,
}

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

impl crate::request_builder::ZkvmOps for Risc0ZkvmOps {
    fn executor(&self) -> Arc<dyn crate::request_builder::LocalExecutor + Sync + Send> {
        self.local_executor.clone()
    }

    fn evaluator(&self) -> Arc<dyn crate::prover_utils::RequestEvaluator + Sync + Send> {
        self.evaluator.clone()
    }
}
