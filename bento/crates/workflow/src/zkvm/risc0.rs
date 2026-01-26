// Copyright 2026 Boundless Foundation, Inc.
//
// Use of this source code is governed by the Business Source License
// as found in the LICENSE-BSL file.

use anyhow::{Context, Result};
use serde_json::Value as JsonValue;
use taskdb::ReadyTask;
use workflow_common::TaskType;

use crate::{Agent, tasks};

use super::{ZkvmPlugin, ZkvmPluginRegistration};

pub struct Risc0Plugin;

#[async_trait::async_trait(?Send)]
impl ZkvmPlugin for Risc0Plugin {
    fn id(&self) -> &'static str {
        "risc0"
    }

    fn priority(&self) -> i32 {
        100
    }

    fn can_handle(&self, task_type: &TaskType) -> bool {
        matches!(
            task_type,
            TaskType::Executor(_)
                | TaskType::Prove(_)
                | TaskType::Join(_)
                | TaskType::Resolve(_)
                | TaskType::Finalize(_)
                | TaskType::Snark(_)
                | TaskType::Keccak(_)
                | TaskType::Union(_)
        )
    }

    fn init(&self, agent: &Agent) -> Result<()> {
        // RISC0 initialization is already handled in Agent::new() today.
        // This hook exists so future refactors can move state behind plugin init.
        let _ = agent;
        Ok(())
    }

    async fn handle(&self, agent: &Agent, task: &ReadyTask, task_type: TaskType) -> Result<JsonValue> {
        match task_type {
            TaskType::Executor(req) => serde_json::to_value(
                tasks::executor::executor(agent, &task.job_id, &req)
                    .await
                    .context("[BENTO-WF-R0-001] Executor failed")?,
            )
            .context("[BENTO-WF-R0-002] Failed to serialize executor response"),

            TaskType::Prove(req) => serde_json::to_value(
                tasks::prove::prover(agent, &task.job_id, &task.task_id, &req).await?,
            )
            .context("[BENTO-WF-R0-003] Failed to serialize prove response"),

            TaskType::Join(req) => {
                if agent.is_povw_enabled() {
                    serde_json::to_value(
                        tasks::join_povw::join_povw(agent, &task.job_id, &req).await?,
                    )
                    .context("[BENTO-WF-R0-004] Failed to serialize povw join response")
                } else {
                    serde_json::to_value(tasks::join::join(agent, &task.job_id, &req).await?)
                    .context("[BENTO-WF-R0-005] Failed to serialize join response")
                }
            }

            TaskType::Resolve(req) => {
                if agent.is_povw_enabled() {
                    serde_json::to_value(tasks::resolve_povw::resolve_povw(agent, &task.job_id, &req).await?)
                    .context("[BENTO-WF-R0-006] Failed to serialize povw resolve response")
                } else {
                    serde_json::to_value(tasks::resolve::resolver(agent, &task.job_id, &req).await?)
                    .context("[BENTO-WF-R0-007] Failed to serialize resolve response")
                }
            }

            TaskType::Finalize(req) => {
                serde_json::to_value(tasks::finalize::finalize(agent, &task.job_id, &req).await?)
            }
            .context("[BENTO-WF-R0-008] Failed to serialize finalize response"),

            TaskType::Snark(req) => serde_json::to_value(
                tasks::snark::stark2snark(agent, &task.job_id.to_string(), &req).await?,
            )
            .context("[BENTO-WF-R0-009] failed to serialize snark response"),

            TaskType::Keccak(req) => serde_json::to_value(
                tasks::keccak::keccak(agent, &task.job_id, &task.task_id, &req).await?,
            )
            .context("[BENTO-WF-R0-010] failed to serialize keccak response"),

            TaskType::Union(req) => serde_json::to_value(tasks::union::union(agent, &task.job_id, &req).await?)
            .context("[BENTO-WF-R0-011] failed to serialize union response"),
            TaskType::Plugin(_) => unreachable!("can_handle() excludes plugin tasks"),
        }
    }
}

inventory::submit!(ZkvmPluginRegistration { plugin: &Risc0Plugin });

