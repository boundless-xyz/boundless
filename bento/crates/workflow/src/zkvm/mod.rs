// Copyright 2026 Boundless Foundation, Inc.
//
// Use of this source code is governed by the Business Source License
// as found in the LICENSE-BSL file.

//! zkVM plugin registry + dispatch glue.
//!
//! Design goals:
//! - Core `workflow` task loop does not need to change when adding a new zkVM.
//! - zkVM implementations are feature-gated modules that self-register.

use anyhow::{Context, Result};
use serde_json::Value as JsonValue;
use taskdb::ReadyTask;
use workflow_common::TaskType;

use crate::Agent;

/// A zkVM plugin that can initialize itself and handle a subset of workflow tasks.
#[async_trait::async_trait(?Send)]
pub trait ZkvmPlugin: Sync + Send {
    /// Stable identifier for CLI selection (e.g. `"risc0"`, `"sp1"`).
    fn id(&self) -> &'static str;

    /// Whether this plugin can handle the given `TaskType`.
    fn can_handle(&self, task_type: &TaskType) -> bool;

    /// Deterministic precedence for `--zkvm auto` if multiple plugins match.
    fn priority(&self) -> i32 {
        0
    }

    /// Lazy initialization hook. Should be idempotent.
    fn init(&self, _agent: &Agent) -> Result<()> {
        Ok(())
    }

    /// Handle a task.
    async fn handle(&self, agent: &Agent, task: &ReadyTask, task_type: TaskType)
        -> Result<JsonValue>;
}

/// Registered plugin entry.
pub struct ZkvmPluginRegistration {
    /// Registered plugin implementation.
    pub plugin: &'static dyn ZkvmPlugin,
}

inventory::collect!(ZkvmPluginRegistration);

/// Return all registered zkVM plugins (deterministic order).
pub fn plugins() -> Vec<&'static dyn ZkvmPlugin> {
    let mut v: Vec<_> = inventory::iter::<ZkvmPluginRegistration>
        .into_iter()
        .map(|r| r.plugin)
        .collect();

    // Ensure deterministic selection regardless of link order.
    v.sort_by(|a, b| b.priority().cmp(&a.priority()).then_with(|| a.id().cmp(b.id())));
    v
}

/// Lookup a plugin by its stable identifier.
pub fn plugin_by_id(id: &str) -> Option<&'static dyn ZkvmPlugin> {
    plugins().into_iter().find(|p| p.id() == id)
}

/// Select a plugin for a task by capability (`--zkvm auto`).
pub fn select_plugin_for_task(task_type: &TaskType) -> Result<&'static dyn ZkvmPlugin> {
    let candidates: Vec<_> = plugins().into_iter().filter(|p| p.can_handle(task_type)).collect();
    match candidates.as_slice() {
        [] => anyhow::bail!("No zkVM plugin registered that can handle task: {:?}", task_type),
        [only] => Ok(*only),
        many => {
            // After sorting in `plugins()`, "first" is deterministic.
            let chosen = many[0];
            tracing::warn!(
                "Multiple zkVM plugins match task; choosing '{}': {}",
                chosen.id(),
                many.iter().map(|p| p.id()).collect::<Vec<_>>().join(", ")
            );
            Ok(chosen)
        }
    }
}

/// Resolve a plugin from `--zkvm <id|auto>` and validate it can handle the task.
pub fn resolve_plugin(selection: &str, task_type: &TaskType) -> Result<&'static dyn ZkvmPlugin> {
    if selection == "auto" {
        return select_plugin_for_task(task_type);
    }

    let plugin = plugin_by_id(selection).with_context(|| {
        let available = plugins().into_iter().map(|p| p.id()).collect::<Vec<_>>().join(", ");
        format!("Unknown zkVM plugin '{selection}'. Available: {available}")
    })?;

    if !plugin.can_handle(task_type) {
        anyhow::bail!(
            "zkVM plugin '{}' cannot handle task {:?}",
            plugin.id(),
            task_type
        );
    }

    Ok(plugin)
}

// Built-in plugins (feature-gated) self-register via inventory.
pub(crate) mod risc0;

