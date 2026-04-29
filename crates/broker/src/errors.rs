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

/// Generates `impl CodedError` and `impl_coded_debug!` for a broker error enum.
///
/// Each generated code follows the pattern `[B-<svc>-<code>]`. Variant patterns
/// are written exactly as they appear in the `match` arm:
/// - tuple variants as `Variant(..)`
/// - struct variants as `Variant { .. }`
/// - unit variants without anything
///
/// # Example
///
/// ```ignore
/// coded_error_impl!(MyErr, "ME",
///     DbError(..)            => "001",
///     ConfigErr(..)          => "002",
///     BelowMinimum { .. }    => "003",
///     NoData                 => "500",
/// );
/// ```
///
/// expands to an `impl CodedError` whose `code()` returns the matching
/// `"[B-ME-NNN]"` string per variant, plus an `impl_coded_debug!` invocation.
#[macro_export]
macro_rules! coded_error_impl {
    (
        $ty:ident, $svc:literal,
        $(
            $variant:ident
            $(( $($_args:tt)* ))?
            $({ $($_fields:tt)* })?
            => $code:literal
        ),+ $(,)?
    ) => {
        impl $crate::errors::CodedError for $ty {
            fn code(&self) -> &str {
                match self {
                    $(
                        Self::$variant
                            $(( $($_args)* ))?
                            $({ $($_fields)* })?
                        => concat!("[B-", $svc, "-", $code, "]")
                    ),+
                }
            }
        }
        $crate::impl_coded_debug!($ty);
    };
}

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

#[cfg(test)]
mod tests {
    use super::CodedError;
    use thiserror::Error;

    #[derive(Error)]
    enum SampleErr {
        #[error("{code} db: {0}", code = self.code())]
        DbError(String),
        #[error("{code} config: {0}", code = self.code())]
        ConfigErr(anyhow::Error),
        #[error(
            "{code} below minimum: have {actual}, need {required}",
            code = self.code()
        )]
        BelowMinimum { actual: u64, required: u64 },
        #[error("{code} no data variant", code = self.code())]
        NoData,
    }

    crate::coded_error_impl!(SampleErr, "TST",
        DbError(..)        => "001",
        ConfigErr(..)      => "002",
        BelowMinimum { .. } => "003",
        NoData             => "500",
    );

    #[test]
    fn coded_error_impl_emits_expected_codes() {
        let db = SampleErr::DbError("boom".to_string());
        let cfg = SampleErr::ConfigErr(anyhow::anyhow!("nope"));
        let strct = SampleErr::BelowMinimum { actual: 1, required: 2 };
        let unit = SampleErr::NoData;

        assert_eq!(db.code(), "[B-TST-001]");
        assert_eq!(cfg.code(), "[B-TST-002]");
        assert_eq!(strct.code(), "[B-TST-003]");
        assert_eq!(unit.code(), "[B-TST-500]");
    }

    #[test]
    fn coded_error_impl_provides_debug_with_code() {
        let err = SampleErr::DbError("boom".to_string());
        let dbg = format!("{:?}", err);
        assert!(dbg.contains("[B-TST-001]"), "debug output missing code: {dbg}");
    }

    #[test]
    fn coded_error_impl_display_includes_code_via_self_code() {
        // Variant attributes use `{code}` bound to `self.code()` — the macro must
        // produce a CodedError impl that returns the right code so Display works.
        let db = SampleErr::DbError("boom".to_string());
        let cfg = SampleErr::ConfigErr(anyhow::anyhow!("nope"));
        let strct = SampleErr::BelowMinimum { actual: 1, required: 2 };
        let unit = SampleErr::NoData;

        assert_eq!(format!("{}", db), "[B-TST-001] db: boom");
        assert_eq!(format!("{}", cfg), "[B-TST-002] config: nope");
        assert_eq!(format!("{}", strct), "[B-TST-003] below minimum: have 1, need 2");
        assert_eq!(format!("{}", unit), "[B-TST-500] no data variant");
    }
}
