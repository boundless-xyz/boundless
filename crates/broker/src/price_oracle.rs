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
use crate::task::{RetryRes, RetryTask, SupervisorErr};
use boundless_market::price_oracle::{PriceOracleError, PriceOracleManager};
use tokio_util::sync::CancellationToken;

impl RetryTask for PriceOracleManager {
    type Error = PriceOracleError;

    fn spawn(&self, cancel_token: CancellationToken) -> RetryRes<Self::Error> {
        let manager = self.clone();
        Box::pin(async move {
            tracing::info!("Starting price oracle refresh task");
            manager.start_oracle(cancel_token).await.map_err(|err| match err {
                PriceOracleError::UpdateTimeout() => {
                    tracing::error!("Price oracle could not refresh prices for too long, if this continues consider setting static prices in the config for `eth_usd` and `zkc_usd`: {}", err);
                    SupervisorErr::Recover(err)
                },
                _ => SupervisorErr::Recover(err),
            })?;
            Ok(())
        })
    }
}

impl CodedError for PriceOracleError {
    fn code(&self) -> &str {
        PriceOracleError::code(self)
    }
}
