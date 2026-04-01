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

pub trait CodedError: std::error::Error {
    fn code(&self) -> &str;
}

// Macro for implementing Debug for CodedError. Ensures the error code is included in the debug output.
#[macro_export]
macro_rules! impl_coded_debug {
    ($name:ident) => {
        use std::backtrace::Backtrace;
        use std::backtrace::BacktraceStatus;
        impl std::fmt::Debug for $name
        where
            $name: CodedError,
        {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                let backtrace = Backtrace::capture();
                let code = self.code();
                // If the code is already included in the message, remove it
                let message = self.to_string().replace(code, "");
                write!(f, "{} {} {}", std::any::type_name::<Self>(), code, message)?;
                // Backtrace status == Captured if RUST_BACKTRACE=true
                if backtrace.status() == BacktraceStatus::Captured {
                    write!(f, "\nBacktrace:\n{}", backtrace)?;
                }
                Ok(())
            }
        }
    };
}

pub use impl_coded_debug;

use boundless_market::telemetry::CompletionOutcome;

use tokio::sync::mpsc;

use crate::config::ConfigLock;
use crate::db::DbObj;
use crate::order_committer::{CommitmentComplete, CommitmentOutcome};
use crate::{Order, OrderStatus};

// Structured broker failure combining an error code with a human-readable reason.
pub(crate) struct BrokerFailure {
    pub code: String,
    pub reason: String,
    pub outcome: CompletionOutcome,
}

impl BrokerFailure {
    pub fn new(
        code: impl Into<String>,
        reason: impl Into<String>,
        outcome: CompletionOutcome,
    ) -> Self {
        Self { code: code.into(), reason: reason.into(), outcome }
    }
}

impl std::fmt::Display for BrokerFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} {}", self.code, self.reason)
    }
}

/// Sets DB failure status, emits a telemetry Failed event, and sends
/// [`CommitmentOutcome::ProvingFailed`] to the OrderCommitter to free the capacity slot.
pub(crate) async fn handle_order_failure(
    db: &DbObj,
    order_id: &str,
    failure: &BrokerFailure,
    chain_id: u64,
    proving_completion_tx: &mpsc::Sender<CommitmentComplete>,
) {
    let db_error_str = failure.to_string();
    if let Err(e) = db.set_order_failure(order_id, &db_error_str).await {
        tracing::error!("Failed to set order {order_id} failure: {e:?}");
    }
    crate::telemetry::telemetry(chain_id).record_failed(order_id, failure);
    let _ = proving_completion_tx.try_send(CommitmentComplete {
        order_id: order_id.to_string(),
        chain_id,
        outcome: CommitmentOutcome::ProvingFailed,
    });
}

/// Cancels an in-progress proof (if configured) then delegates to [`handle_order_failure`]
/// to set DB status, emit telemetry, and free the OrderCommitter capacity slot.
pub(crate) async fn cancel_proof_and_fail(
    prover: &crate::provers::ProverObj,
    db: &DbObj,
    config: &ConfigLock,
    order: &Order,
    failure: &BrokerFailure,
    chain_id: u64,
    proving_completion_tx: &mpsc::Sender<CommitmentComplete>,
) {
    let order_id = order.id();

    let should_cancel = match config.lock_all() {
        Ok(conf) => conf.market.cancel_proving_expired_orders,
        Err(err) => {
            tracing::warn!(
                "[B-UTL-002] Failed to read config for cancellation decision; skipping cancel: {err:?}"
            );
            false
        }
    };

    if should_cancel {
        if let Some(proof_id) = order.proof_id.as_ref() {
            if matches!(order.status, OrderStatus::Proving) {
                tracing::debug!("Cancelling proof {} for order {}", proof_id, order_id);
                if let Err(err) = prover.cancel_stark(proof_id).await {
                    tracing::warn!(
                        "[B-UTL-001] Failed to cancel proof {proof_id} with reason: {} for order {order_id}: {err}",
                        failure.code
                    );
                }
            }
        }
    }

    handle_order_failure(db, &order_id, failure, chain_id, proving_completion_tx).await;
}
