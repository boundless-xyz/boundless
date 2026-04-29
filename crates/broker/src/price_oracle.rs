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

use crate::errors::CodedError;
use crate::task::{BrokerService, SupervisorErr};
use boundless_market::price_oracle::{PriceOracleError, PriceOracleManager};
use tokio_util::sync::CancellationToken;

impl BrokerService for PriceOracleManager {
    type Error = PriceOracleError;

    async fn run(self, cancel_token: CancellationToken) -> Result<(), SupervisorErr<Self::Error>> {
        tracing::info!("Starting price oracle refresh task");
        self.start_oracle(cancel_token).await.map_err(SupervisorErr::Recover)
    }
}

// `coded_error_impl!` doesn't fit here: PriceOracleError is a foreign type
// from `boundless_market::price_oracle` and its variants live upstream.
// Delegate to the upstream-provided `code()` method instead.
impl CodedError for PriceOracleError {
    fn code(&self) -> &str {
        PriceOracleError::code(self)
    }
}
