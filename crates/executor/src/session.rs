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

//! Session lifecycle for Bonsai-compatible execute requests.
//!
//! A session is created by `POST /sessions/create`. We immediately spawn a
//! background task that resolves the referenced image + input, dispatches to
//! the configured [`Executor`], and records the outcome. Clients poll
//! `GET /sessions/status/{uuid}` until the state leaves `RUNNING`.
//!
//! Only `execute_only = true` sessions are supported — we do not ship a
//! prover, so proving requests are rejected at creation time.

use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};

use tokio::sync::RwLock;
use uuid::Uuid;

use crate::{
    backend::{ExecutionStats, ExecutorError, Registry, ZkvmKind},
    storage::Storage,
};

/// Live state of a session. Exposed via the `/sessions/status/{uuid}` endpoint.
#[derive(Debug, Clone)]
pub enum SessionPhase {
    Running { stage: &'static str },
    Succeeded { stats: ExecutionStats },
    Failed { error: String },
    Aborted,
}

impl SessionPhase {
    pub fn status_str(&self) -> &'static str {
        match self {
            SessionPhase::Running { .. } => "RUNNING",
            SessionPhase::Succeeded { .. } => "SUCCEEDED",
            SessionPhase::Failed { .. } => "FAILED",
            SessionPhase::Aborted => "ABORTED",
        }
    }
}

/// Per-session record kept in [`Storage`].
pub struct SessionRecord {
    pub uuid: String,
    pub img: String,
    pub input: String,
    pub zkvm: ZkvmKind,
    pub execute_only: bool,
    pub created_at: Instant,
    phase: RwLock<SessionPhase>,
    logs: RwLock<String>,
    /// Atomic so it's cheap to poll from inside the blocking executor thread
    /// without touching any tokio primitive.
    aborted: AtomicBool,
}

impl SessionRecord {
    pub fn new(img: String, input: String, zkvm: ZkvmKind, execute_only: bool) -> Arc<Self> {
        Arc::new(Self {
            uuid: Uuid::new_v4().to_string(),
            img,
            input,
            zkvm,
            execute_only,
            created_at: Instant::now(),
            phase: RwLock::new(SessionPhase::Running { stage: "Setup" }),
            logs: RwLock::new(String::new()),
            aborted: AtomicBool::new(false),
        })
    }

    pub async fn phase(&self) -> SessionPhase {
        self.phase.read().await.clone()
    }

    pub async fn logs(&self) -> String {
        self.logs.read().await.clone()
    }

    pub async fn elapsed(&self) -> Duration {
        // For terminated sessions we freeze elapsed at the completion point
        // by recording it in the SUCCEEDED stats. For running sessions we
        // report wall time since creation.
        if let SessionPhase::Succeeded { stats } = &*self.phase.read().await {
            return Duration::from_millis(stats.elapsed_ms);
        }
        self.created_at.elapsed()
    }

    pub async fn mark_aborted(&self) {
        self.aborted.store(true, Ordering::Release);
        let mut phase = self.phase.write().await;
        if matches!(&*phase, SessionPhase::Running { .. }) {
            *phase = SessionPhase::Aborted;
        }
    }

    /// Non-blocking, lock-free abort check — safe to call from any thread,
    /// including the `spawn_blocking` executor thread.
    fn is_aborted(&self) -> bool {
        self.aborted.load(Ordering::Acquire)
    }

    async fn set_stage(&self, stage: &'static str) {
        let mut phase = self.phase.write().await;
        if matches!(&*phase, SessionPhase::Running { .. }) {
            *phase = SessionPhase::Running { stage };
        }
    }

    async fn set_phase(&self, next: SessionPhase) {
        *self.phase.write().await = next;
    }

    async fn append_log(&self, line: impl AsRef<str>) {
        let mut logs = self.logs.write().await;
        logs.push_str(line.as_ref());
        logs.push('\n');
    }
}

/// Spawn a background task that executes this session and records the result.
pub fn spawn_session(session: Arc<SessionRecord>, storage: Storage, registry: Arc<Registry>) {
    tokio::spawn(async move {
        let outcome = run_session(session.clone(), storage, registry).await;
        match outcome {
            Ok(stats) => {
                session
                    .append_log(format!(
                        "execution succeeded: {} total cycles, {} user cycles",
                        stats.total_cycles, stats.user_cycles
                    ))
                    .await;
                session.set_phase(SessionPhase::Succeeded { stats }).await;
            }
            Err(err) => {
                let msg = err.to_string();
                session.append_log(format!("execution failed: {msg}")).await;
                // Don't overwrite an explicit abort.
                let is_aborted = matches!(&*session.phase.read().await, SessionPhase::Aborted);
                if !is_aborted {
                    session.set_phase(SessionPhase::Failed { error: msg }).await;
                }
            }
        }
    });
}

async fn run_session(
    session: Arc<SessionRecord>,
    storage: Storage,
    registry: Arc<Registry>,
) -> Result<ExecutionStats, ExecutorError> {
    session
        .append_log(format!(
            "session {} created (zkvm={}, img={}, input={})",
            session.uuid, session.zkvm, session.img, session.input
        ))
        .await;

    session.set_stage("Setup").await;

    let elf = storage
        .get_image(&session.img)
        .ok_or_else(|| ExecutorError::InvalidElf(format!("unknown image id: {}", session.img)))?;
    let input = storage.get_input(&session.input).ok_or_else(|| {
        ExecutorError::InvalidInput(format!("unknown input id: {}", session.input))
    })?;

    let backend = registry.get(session.zkvm).ok_or(ExecutorError::BackendDisabled(session.zkvm))?;

    session.set_stage("Executor").await;

    // Short-circuit if someone called /sessions/stop between create and now.
    if session.is_aborted() {
        return Err(ExecutorError::Execution(anyhow::anyhow!("session aborted")));
    }

    let session_for_blocking = session.clone();
    tokio::task::spawn_blocking(move || {
        if session_for_blocking.is_aborted() {
            return Err(ExecutorError::Execution(anyhow::anyhow!("session aborted")));
        }
        backend.execute(&elf, &input)
    })
    .await
    .map_err(|e| ExecutorError::Execution(anyhow::anyhow!("join error: {e}")))?
}
