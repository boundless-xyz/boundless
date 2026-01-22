// Copyright 2026 Boundless Foundation, Inc.
//
// Use of this source code is governed by the Business Source License
// as found in the LICENSE-BSL file.

//! SP1 prove task handler
//!
//! This module handles SP1 proving as a single job (similar to snark).
//! It takes an ELF and input, executes and proves with SP1, and returns a proof.

use crate::Agent;
use anyhow::{Context, Result};
use std::time::Instant;
use uuid::Uuid;
use workflow_common::{
    Sp1Req, Sp1Resp,
    metrics::helpers,
    s3::{ELF_BUCKET_DIR, INPUT_BUCKET_DIR, RECEIPT_BUCKET_DIR, STARK_BUCKET_DIR},
};

/// SP1 prove task - executes and proves a program in one go
pub async fn prove_sp1(agent: &Agent, job_id: &Uuid, request: &Sp1Req) -> Result<Sp1Resp> {
    let start_time = Instant::now();
    tracing::info!("Starting SP1 prove for job: {job_id}");

    // Fetch ELF binary data
    let elf_key = format!("{ELF_BUCKET_DIR}/{}", request.image);
    tracing::debug!("Downloading ELF - {}", elf_key);
    let s3_read_start = Instant::now();
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
    let input_key = format!("{INPUT_BUCKET_DIR}/{}", request.input);
    tracing::debug!("Downloading input - {}", input_key);
    let input_s3_start = Instant::now();
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
    let prove_start = Instant::now();

    // Setup SP1 prover client
    let client = sp1_sdk::ProverClient::from_env();

    // Setup the inputs
    let mut stdin = sp1_sdk::SP1Stdin::new();
    stdin.write_slice(&input_data);

    // Setup the program for proving (generates proving key and verification key)
    tracing::debug!("Setting up SP1 program for proving");
    let (pk, vk) = client.setup(&elf_data);

    // Generate the compressed proof
    tracing::debug!("Generating SP1 proof");
    let proof = client
        .prove(&pk, &stdin)
        .compressed()
        .run()
        .map_err(|e| anyhow::anyhow!("SP1 proof generation failed: {:?}", e))?;

    tracing::debug!("SP1 proof generated successfully");

    // Verify the proof
    tracing::debug!("Verifying SP1 proof");
    client
        .verify(&proof, &vk)
        .map_err(|e| anyhow::anyhow!("SP1 proof verification failed: {:?}", e))?;

    tracing::debug!("SP1 proof verified successfully");

    // Serialize the proof for storage
    // SP1 proof implements Serialize, so we can use bincode
    let proof_bytes = bincode::serialize(&proof)
        .context("Failed to serialize SP1 proof")?;

    let prove_elapsed = prove_start.elapsed().as_secs_f64();
    helpers::record_task_operation("sp1", "prove", "success", prove_elapsed);

    // Write proof to S3
    let proof_key = format!("{RECEIPT_BUCKET_DIR}/{STARK_BUCKET_DIR}/{}.bincode", job_id);
    tracing::info!("Uploading SP1 proof to S3: {proof_key}");

    let s3_write_start = Instant::now();
    match agent.s3_client.write_buf_to_s3(&proof_key, proof_bytes).await {
        Ok(()) => {
            helpers::record_s3_operation(
                "write",
                "success",
                s3_write_start.elapsed().as_secs_f64(),
            );
        }
        Err(e) => {
            helpers::record_s3_operation("write", "error", s3_write_start.elapsed().as_secs_f64());
            return Err(e.context("Failed to upload SP1 proof to obj store"));
        }
    }

    // Record total task duration and success
    helpers::record_task_operation(
        "sp1",
        "complete",
        "success",
        start_time.elapsed().as_secs_f64(),
    );

    Ok(Sp1Resp {
        proof: job_id.to_string(),
    })
}
