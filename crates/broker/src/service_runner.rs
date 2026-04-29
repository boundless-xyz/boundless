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

//! Builder for registering supervised broker services.
//!
//! Replaces the ~6-line `JoinSet::spawn(async move { Supervisor::new(...) ... })`
//! block that is otherwise duplicated 18 times in `start_service` and
//! `start_chain_pipeline`. Each registration is now a single
//! `runner.spawn(service, Criticality::X, "label")` call.

use std::{future::Future, sync::Arc};

use anyhow::{Context, Result};
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;
use tracing::{Instrument, Span};

use crate::{
    config::ConfigLock,
    task::{RetryPolicy, RetryTask, Supervisor},
};

/// Classifies a supervised service for shutdown ordering and retry behaviour.
///
/// Critical and non-critical tasks live on separate `JoinSet`s and use
/// independent `CancellationToken`s — the shutdown protocol cancels
/// non-critical tasks first (to stop taking new work), drains in-flight
/// commitments, then cancels critical tasks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Criticality {
    /// Critical task, supervised with the default [`RetryPolicy`].
    Critical,
    /// Critical task, supervised with [`RetryPolicy::CRITICAL_SERVICE`]
    /// (faster retry cadence, capped at `max_critical_task_retries`).
    CriticalWithFastRetry,
    /// Non-critical task, supervised with the default [`RetryPolicy`].
    NonCritical,
}

/// Decomposed parts of a [`ServiceRunner`], suitable for plugging into the
/// existing two-phase shutdown loop in `Broker::start_service`.
pub struct ServiceRunnerParts {
    pub non_critical_tasks: JoinSet<Result<()>>,
    pub critical_tasks: JoinSet<Result<()>>,
    pub non_critical_cancel_token: CancellationToken,
    pub critical_cancel_token: CancellationToken,
}

/// Owns the JoinSets and cancellation tokens for the broker's supervised
/// services and provides one-line registration via [`ServiceRunner::spawn`].
pub struct ServiceRunner {
    config: ConfigLock,
    critical_tasks: JoinSet<Result<()>>,
    non_critical_tasks: JoinSet<Result<()>>,
    critical_cancel_token: CancellationToken,
    non_critical_cancel_token: CancellationToken,
}

impl ServiceRunner {
    /// Creates a new runner with empty JoinSets and fresh cancellation tokens.
    pub fn new(config: ConfigLock) -> Self {
        Self {
            config,
            critical_tasks: JoinSet::new(),
            non_critical_tasks: JoinSet::new(),
            critical_cancel_token: CancellationToken::new(),
            non_critical_cancel_token: CancellationToken::new(),
        }
    }

    /// Cancellation token for non-critical tasks. Useful when a caller needs
    /// to spawn a raw future via [`Self::spawn_future_in_span`] that consumes
    /// the token.
    pub fn non_critical_cancel_token(&self) -> CancellationToken {
        self.non_critical_cancel_token.clone()
    }

    /// Registers a supervised service. Equivalent to the legacy
    /// `tasks.spawn(async move { Supervisor::new(...) ... })` block.
    pub fn spawn<T>(&mut self, service: Arc<T>, criticality: Criticality, label: &'static str)
    where
        T: RetryTask + Send + Sync + 'static,
        T::Error: Send + Sync + 'static,
    {
        self.spawn_in_span(service, criticality, label, Span::current());
    }

    /// Same as [`Self::spawn`] but instruments the supervisor future with the
    /// given span — used in `start_chain_pipeline` where each chain's services
    /// run inside a per-chain `info_span!`.
    pub fn spawn_in_span<T>(
        &mut self,
        service: Arc<T>,
        criticality: Criticality,
        label: &'static str,
        span: Span,
    ) where
        T: RetryTask + Send + Sync + 'static,
        T::Error: Send + Sync + 'static,
    {
        let config = self.config.clone();
        let (joinset, cancel_token, retry_policy) = self.dispatch(criticality);
        let supervisor =
            Supervisor::new(service, config, cancel_token).with_retry_policy(retry_policy);
        joinset.spawn(
            async move { supervisor.spawn().await.with_context(|| format!("Failed to run {label}")) }
                .instrument(span),
        );
    }

    /// Registers a raw future that performs its own cancellation handling
    /// (e.g. the telemetry service, which doesn't go through `Supervisor`).
    /// Instruments the future with the given span — used in
    /// `start_chain_pipeline` where each chain's services run inside a
    /// per-chain `info_span!`.
    pub fn spawn_future_in_span<F>(
        &mut self,
        criticality: Criticality,
        label: &'static str,
        span: Span,
        fut: F,
    ) where
        F: Future<Output = Result<()>> + Send + 'static,
    {
        let (joinset, _cancel_token, _retry_policy) = self.dispatch(criticality);
        joinset.spawn(
            async move { fut.await.with_context(|| format!("Failed to run {label}")) }
                .instrument(span),
        );
    }

    /// Decomposes the runner into the JoinSets and tokens consumed by the
    /// existing shutdown loop. After this call the runner cannot register
    /// new services — that's intentional, registration happens at startup
    /// and shutdown handling takes over.
    pub fn into_parts(self) -> ServiceRunnerParts {
        ServiceRunnerParts {
            non_critical_tasks: self.non_critical_tasks,
            critical_tasks: self.critical_tasks,
            non_critical_cancel_token: self.non_critical_cancel_token,
            critical_cancel_token: self.critical_cancel_token,
        }
    }

    fn dispatch(
        &mut self,
        criticality: Criticality,
    ) -> (&mut JoinSet<Result<()>>, CancellationToken, RetryPolicy) {
        match criticality {
            Criticality::Critical => (
                &mut self.critical_tasks,
                self.critical_cancel_token.clone(),
                RetryPolicy::default(),
            ),
            Criticality::CriticalWithFastRetry => (
                &mut self.critical_tasks,
                self.critical_cancel_token.clone(),
                RetryPolicy::CRITICAL_SERVICE,
            ),
            Criticality::NonCritical => (
                &mut self.non_critical_tasks,
                self.non_critical_cancel_token.clone(),
                RetryPolicy::default(),
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        errors::CodedError,
        task::{BrokerService, SupervisorErr},
    };
    use std::sync::atomic::{AtomicUsize, Ordering};
    use thiserror::Error;

    #[derive(Error, Debug)]
    enum TestErr {
        #[error("test")]
        _Boom,
    }

    impl CodedError for TestErr {
        fn code(&self) -> &str {
            "[B-TEST-001]"
        }
    }

    #[derive(Clone)]
    struct CountingService {
        counter: Arc<AtomicUsize>,
    }

    impl BrokerService for CountingService {
        type Error = TestErr;

        async fn run(
            self,
            cancel_token: CancellationToken,
        ) -> Result<(), SupervisorErr<Self::Error>> {
            self.counter.fetch_add(1, Ordering::SeqCst);
            cancel_token.cancelled().await;
            Ok(())
        }
    }

    #[tokio::test]
    async fn cancel_tokens_are_independent() {
        let runner = ServiceRunner::new(ConfigLock::default());
        let non_crit = runner.non_critical_cancel_token();
        let parts = runner.into_parts();

        non_crit.cancel();
        assert!(non_crit.is_cancelled());
        assert!(
            !parts.critical_cancel_token.is_cancelled(),
            "non-critical cancel must not affect critical"
        );
    }

    #[tokio::test]
    async fn spawn_routes_to_critical_joinset_and_cancels_cleanly() {
        let mut runner = ServiceRunner::new(ConfigLock::default());
        let counter = Arc::new(AtomicUsize::new(0));
        let svc = Arc::new(CountingService { counter: counter.clone() });

        runner.spawn(svc, Criticality::Critical, "Counter");
        let mut parts = runner.into_parts();

        // Wait until the service has actually started running.
        while counter.load(Ordering::SeqCst) == 0 {
            tokio::task::yield_now().await;
        }
        assert!(parts.non_critical_tasks.is_empty(), "should be in critical joinset");

        parts.critical_cancel_token.cancel();
        let res = parts.critical_tasks.join_next().await.unwrap().unwrap();
        assert!(res.is_ok(), "service should exit cleanly on cancel: {res:?}");
    }

    #[tokio::test]
    async fn spawn_future_runs_to_completion() {
        let mut runner = ServiceRunner::new(ConfigLock::default());
        let cancel = runner.non_critical_cancel_token();

        runner.spawn_future_in_span(
            Criticality::NonCritical,
            "Echo",
            tracing::Span::current(),
            async move {
                cancel.cancelled().await;
                Ok(())
            },
        );

        let non_crit_cancel = runner.non_critical_cancel_token();
        let mut parts = runner.into_parts();
        non_crit_cancel.cancel();
        let res = parts.non_critical_tasks.join_next().await.unwrap().unwrap();
        assert!(res.is_ok());
    }
}
