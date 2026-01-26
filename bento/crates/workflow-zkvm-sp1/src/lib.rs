// Copyright 2026 Boundless Foundation, Inc.
//
// Use of this source code is governed by the Business Source License
// as found in the LICENSE-BSL file.

//! SP1 zkVM plugin for the Bento workflow agent.
//!
//! This crate is intentionally **self-contained**:
//! - It registers itself into the `workflow::zkvm` inventory registry.
//! - It defines its own request schema inside the plugin payload.
//! - The core `workflow` crate does not need to be modified to add new zkVM plugins.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use taskdb::ReadyTask;
use uuid::Uuid;
use workflow::zkvm::ZkvmPlugin;
use workflow_common::{
    TaskType,
    metrics::helpers,
    s3::{ELF_BUCKET_DIR, INPUT_BUCKET_DIR, RECEIPT_BUCKET_DIR, STARK_BUCKET_DIR},
};

#[derive(Debug, Deserialize, Serialize)]
struct Sp1ProvePayload {
    /// Image / ELF key in object store
    image: String,
    /// Input key in object store
    input: String,
}

pub struct Sp1Plugin;

#[async_trait::async_trait(?Send)]
impl ZkvmPlugin for Sp1Plugin {
    fn id(&self) -> &'static str {
        "sp1"
    }

    fn can_handle(&self, task_type: &TaskType) -> bool {
        matches!(
            task_type,
            TaskType::Plugin(t) if t.plugin == "sp1"
        )
    }

    async fn handle(
        &self,
        agent: &workflow::Agent,
        task: &ReadyTask,
        task_type: TaskType,
    ) -> Result<JsonValue> {
        match task_type {
            TaskType::Plugin(t) if t.plugin == "sp1" && t.task == "prove" => {
                let payload: Sp1ProvePayload = serde_json::from_value(t.payload)
                    .context("[BENTO-WF-SP1-000] Invalid SP1 payload")?;
                let proof_id = prove_sp1(agent, &task.job_id, payload).await?;
                Ok(serde_json::json!({ "proof": proof_id }))
            }
            TaskType::Plugin(t) if t.plugin == "sp1" => {
                anyhow::bail!("Unknown SP1 task '{}'", t.task)
            }
            _ => unreachable!("can_handle() excludes non-SP1 tasks"),
        }
    }
}

inventory::submit! {
    workflow::zkvm::ZkvmPluginRegistration { plugin: &Sp1Plugin }
}

async fn prove_sp1(agent: &workflow::Agent, job_id: &Uuid, payload: Sp1ProvePayload) -> Result<String> {
    let start_time = std::time::Instant::now();
    tracing::info!("Starting SP1 prove for job: {job_id}");

    // Fetch ELF binary data
    let elf_key = format!("{ELF_BUCKET_DIR}/{}", payload.image);
    tracing::debug!("Downloading ELF - {}", elf_key);
    let s3_read_start = std::time::Instant::now();
    let elf_data = match agent.s3_client.read_buf_from_s3(&elf_key).await {
        Ok(data) => {
            helpers::record_s3_operation("read", "success", s3_read_start.elapsed().as_secs_f64());
            data
        }
        Err(e) => {
            helpers::record_s3_operation("read", "error", s3_read_start.elapsed().as_secs_f64());
            return Err(e.context("Failed to download ELF from obj store"));
        }
    };

    // Fetch input data
    let input_key = format!("{INPUT_BUCKET_DIR}/{}", payload.input);
    tracing::debug!("Downloading input - {}", input_key);
    let input_s3_start = std::time::Instant::now();
    let input_data = match agent.s3_client.read_buf_from_s3(&input_key).await {
        Ok(data) => {
            helpers::record_s3_operation("read", "success", input_s3_start.elapsed().as_secs_f64());
            data
        }
        Err(e) => {
            helpers::record_s3_operation("read", "error", input_s3_start.elapsed().as_secs_f64());
            return Err(e.context("Failed to download input from obj store"));
        }
    };

    // Execute and prove with SP1
    tracing::debug!("Executing and proving with SP1 for job: {job_id}");
    let prove_start = std::time::Instant::now();

    let client = sp1_sdk::ProverClient::from_env();
    let mut stdin = sp1_sdk::SP1Stdin::new();
    stdin.write_slice(&input_data);

    tracing::debug!("Setting up SP1 program for proving");
    let (pk, vk) = client.setup(&elf_data);

    tracing::debug!("Generating SP1 proof");
    let proof = client
        .prove(&pk, &stdin)
        .compressed()
        .run()
        .map_err(|e| anyhow::anyhow!("SP1 proof generation failed: {:?}", e))?;

    tracing::debug!("Verifying SP1 proof");
    client
        .verify(&proof, &vk)
        .map_err(|e| anyhow::anyhow!("SP1 proof verification failed: {:?}", e))?;

    let proof_bytes = bincode::serialize(&proof).context("Failed to serialize SP1 proof")?;
    helpers::record_task_operation("sp1", "prove", "success", prove_start.elapsed().as_secs_f64());

    // Write proof to S3 (align with existing receipt storage convention)
    let proof_key = format!("{RECEIPT_BUCKET_DIR}/{STARK_BUCKET_DIR}/{}.bincode", job_id);
    tracing::info!("Uploading SP1 proof to S3: {proof_key}");

    let s3_write_start = std::time::Instant::now();
    match agent.s3_client.write_buf_to_s3(&proof_key, proof_bytes).await {
        Ok(()) => {
            helpers::record_s3_operation("write", "success", s3_write_start.elapsed().as_secs_f64());
        }
        Err(e) => {
            helpers::record_s3_operation("write", "error", s3_write_start.elapsed().as_secs_f64());
            return Err(e.context("Failed to upload SP1 proof to obj store"));
        }
    }

    helpers::record_task_operation("sp1", "complete", "success", start_time.elapsed().as_secs_f64());
    Ok(job_id.to_string())
}

