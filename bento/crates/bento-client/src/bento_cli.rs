// Copyright 2026 Boundless Foundation, Inc.
//
// Use of this source code is governed by the Business Source License
// as found in the LICENSE-BSL file.

use anyhow::{Context, Result, bail};
use bonsai_sdk::non_blocking::Client as ProvingClient;
use clap::Parser;
use risc0_zkvm::{Receipt, compute_image_id, serde::to_vec};
use sample_guest_common::IterReq;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
pub struct Args {
    /// Risc0 ZKVM elf file on disk
    #[clap(short = 'f', long)]
    elf_file: Option<PathBuf>,

    /// ZKVM encoded input to be supplied to ExecEnv .write() method
    ///
    /// Should be `risc0_zkvm::serde::to_vec` encoded binary data
    #[clap(short, long, conflicts_with = "iter_count")]
    input_file: Option<PathBuf>,

    /// Optional test vector to run the sample guest with the supplied iteration count
    ///
    /// Allows for rapid testing of arbitrary large cycle count guests
    ///
    /// NOTE: TODO remove this flag and simplify client
    #[clap(short = 'c', long, conflicts_with = "input_file")]
    iter_count: Option<u64>,

    /// Run a execute only job, aka preflight
    ///
    /// Useful for capturing metrics on a STARK proof like cycles.
    #[clap(short, long, default_value_t = false)]
    exec_only: bool,

    /// Bento HTTP API Endpoint
    #[clap(short = 't', long, default_value = "http://localhost:8081")]
    endpoint: String,

    /// Reserved worker count for job processing.
    ///
    /// Sets the reserved worker count for task scheduling. Jobs with higher
    /// reserved values get higher priority in cases where there are more than one task being
    /// progressed at a time.
    /// Default: 0
    #[clap(short, long, default_value_t = 0)]
    reserved: i32,

    /// Use SP1 proving instead of RISC Zero STARK
    #[clap(long, default_value_t = false)]
    sp1: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let args = Args::parse();

    // Format API key with reserved value if specified
    let api_key = if args.reserved != 0 {
        tracing::info!("Using reserved: {}", args.reserved);
        format!("v1:reserved:{}", args.reserved)
    } else {
        String::new()
    };

    let client = ProvingClient::from_parts(args.endpoint.clone(), api_key.clone(), risc0_zkvm::VERSION).unwrap();

    let (image, input) = if let Some(elf_file) = args.elf_file {
        let image = std::fs::read(elf_file).context("Failed to read elf file from disk")?;
        let input = std::fs::read(
            args.input_file.expect("if --elf-file is supplied, supply a --input-file"),
        )?;
        (image, input)
    } else if let Some(iter_count) = args.iter_count {
        let input = to_vec(&IterReq::Iter(iter_count)).expect("Failed to r0 to_vec");
        let input = bytemuck::cast_slice(&input).to_vec();
        (sample_guest_methods::METHOD_NAME_ELF.to_vec(), input)
    } else {
        bail!("Invalid arg config, either elf_file or iter_count should be supplied");
    };

    if args.sp1 {
        // Execute SP1 workflow
        let (_job_id, _proof_id) = sp1_workflow(&args.endpoint, &api_key, image, input).await?;
    } else {
        // Execute STARK workflow
        let (_session_uuid, _receipt_id) =
            stark_workflow(&client, image.clone(), input, vec![], args.exec_only).await?;

        // return if exec only and success
        if args.exec_only {
            return Ok(());
        }
    }

    Ok(())
}

async fn stark_workflow(
    client: &ProvingClient,
    image: Vec<u8>,
    input: Vec<u8>,
    assumptions: Vec<String>,
    exec_only: bool,
) -> Result<(String, String)> {
    // elf/image
    let image_id = compute_image_id(&image).unwrap();
    let image_id_str = image_id.to_string();
    client.upload_img(&image_id_str, image).await.context("Failed to upload image")?;

    // input
    let input_id = client.upload_input(input).await.context("Failed to upload input")?;

    tracing::info!("image_id: {image_id} | input_id: {input_id}");

    let session = client
        .create_session(image_id_str.clone(), input_id, assumptions, exec_only)
        .await
        .context("STARK proof failure")?;
    tracing::info!("STARK job_id: {}", session.uuid);

    let mut receipt_id = String::new();

    loop {
        let res = session.status(client).await.context("Failed to get STARK status")?;

        match res.status.as_ref() {
            "RUNNING" => {
                tracing::info!("STARK Job running....");
                tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;
                continue;
            }
            "SUCCEEDED" => {
                tracing::info!("Job done!");
                if exec_only {
                    break;
                }
                let receipt_bytes = client
                    .receipt_download(&session)
                    .await
                    .context("Failed to download receipt")?;

                let receipt: Receipt = bincode::deserialize(&receipt_bytes).unwrap();
                receipt.verify(image_id).unwrap();

                receipt_id = client
                    .upload_receipt(receipt_bytes.clone())
                    .await
                    .context("Failed to upload receipt")?;

                break;
            }
            _ => {
                bail!(
                    "Job failed: {} - {}",
                    session.uuid,
                    res.error_msg.as_ref().unwrap_or(&String::new())
                );
            }
        }
    }
    Ok((session.uuid, receipt_id))
}

#[derive(Debug, Serialize)]
struct Sp1CreateReq {
    image: String,
    input: String,
}

#[derive(Debug, Deserialize)]
struct Sp1CreateResp {
    uuid: String,
}

#[derive(Debug, Deserialize)]
struct Sp1StatusResp {
    status: String,
    error_msg: Option<String>,
    output: Option<String>,
}

async fn sp1_workflow(
    endpoint: &str,
    api_key: &str,
    image: Vec<u8>,
    input: Vec<u8>,
) -> Result<(String, String)> {
    let client = reqwest::Client::new();

    // Upload image
    // SP1 uses RISC-V ELF format, not RISC Zero format
    // Use SHA256 hash instead of RISC Zero's compute_image_id
    let mut hasher = Sha256::new();
    hasher.update(&image);
    let image_hash = hasher.finalize();
    let image_id_str = hex::encode(image_hash);

    // Upload image - PUT directly (API validates image_id matches hash)
    let image_upload_url = format!("{}/images/upload/{}", endpoint, image_id_str);
    let mut image_upload_req = client.put(&image_upload_url);
    if !api_key.is_empty() {
        image_upload_req = image_upload_req.header("x-api-key", api_key);
    }
    image_upload_req
        .body(image)
        .send()
        .await
        .context("Failed to upload image")?
        .error_for_status()
        .context("Image upload failed")?;

    tracing::info!("Uploaded image: {}", image_id_str);

    // Upload input - first get upload URL
    let input_upload_get_url = format!("{}/inputs/upload", endpoint);
    let mut input_upload_get_req = client.get(&input_upload_get_url);
    if !api_key.is_empty() {
        input_upload_get_req = input_upload_get_req.header("x-api-key", api_key);
    }
    let input_upload_get_resp: serde_json::Value = input_upload_get_req
        .send()
        .await
        .context("Failed to get input upload URL")?
        .error_for_status()
        .context("Failed to get input upload URL")?
        .json()
        .await
        .context("Failed to parse input upload URL response")?;

    let input_id_str = input_upload_get_resp["uuid"]
        .as_str()
        .context("Failed to extract input UUID from response")?
        .to_string();
    let input_upload_url = input_upload_get_resp["url"]
        .as_str()
        .context("Failed to extract input upload URL from response")?
        .to_string();

    // Now upload the input data
    let mut input_upload_req = client.put(&input_upload_url);
    if !api_key.is_empty() {
        input_upload_req = input_upload_req.header("x-api-key", api_key);
    }
    input_upload_req
        .body(input.clone())
        .send()
        .await
        .context("Failed to upload input")?
        .error_for_status()
        .context("Input upload failed")?;

    tracing::info!("Uploaded input: {}", input_id_str);

    // Create SP1 prove job
    let create_url = format!("{}/sp1/create", endpoint);
    let mut create_req = client.post(&create_url);
    if !api_key.is_empty() {
        create_req = create_req.header("x-api-key", api_key);
    }
    let create_resp: Sp1CreateResp = create_req
        .json(&Sp1CreateReq {
            image: image_id_str.clone(),
            input: input_id_str.clone(),
        })
        .send()
        .await
        .context("Failed to create SP1 job")?
        .error_for_status()
        .context("SP1 job creation failed")?
        .json()
        .await
        .context("Failed to parse SP1 job creation response")?;

    let job_id = create_resp.uuid;
    tracing::info!("SP1 job_id: {}", job_id);

    // Poll for status
    let status_url = format!("{}/sp1/status/{}", endpoint, job_id);
    loop {
        let mut status_req = client.get(&status_url);
        if !api_key.is_empty() {
            status_req = status_req.header("x-api-key", api_key);
        }
        let status_resp: Sp1StatusResp = status_req
            .send()
            .await
            .context("Failed to get SP1 status")?
            .error_for_status()
            .context("SP1 status check failed")?
            .json()
            .await
            .context("Failed to parse SP1 status response")?;

        match status_resp.status.as_str() {
            "RUNNING" => {
                tracing::info!("SP1 Job running....");
                tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;
                continue;
            }
            "SUCCEEDED" => {
                tracing::info!("SP1 Job done!");

                // Download proof
                let proof_url = status_resp.output
                    .as_ref()
                    .map(|s| s.clone())
                    .unwrap_or_else(|| format!("{}/receipts/sp1/receipt/{}", endpoint, job_id));

                let mut proof_req = client.get(&proof_url);
                if !api_key.is_empty() {
                    proof_req = proof_req.header("x-api-key", api_key);
                }
                let proof_bytes = proof_req
                    .send()
                    .await
                    .context("Failed to download SP1 proof")?
                    .error_for_status()
                    .context("SP1 proof download failed")?
                    .bytes()
                    .await
                    .context("Failed to read SP1 proof bytes")?;

                tracing::info!("Downloaded SP1 proof ({} bytes)", proof_bytes.len());

                // Save proof to file
                let proof_file = format!("{}.sp1_proof.bin", job_id);
                std::fs::write(&proof_file, &proof_bytes)
                    .context(format!("Failed to write proof to {}", proof_file))?;
                tracing::info!("Saved SP1 proof to: {}", proof_file);

                return Ok((job_id.clone(), job_id));
            }
            "FAILED" => {
                bail!(
                    "SP1 Job failed: {} - {}",
                    job_id,
                    status_resp.error_msg.as_ref().unwrap_or(&String::new())
                );
            }
            _ => {
                bail!("Unknown SP1 job status: {}", status_resp.status);
            }
        }
    }
}
