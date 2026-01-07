// Copyright 2026 Boundless Foundation, Inc.
//
// Use of this source code is governed by the Business Source License
// as found in the LICENSE-BSL file.

use anyhow::{Context, Result, bail};
use bonsai_sdk::non_blocking::Client as ProvingClient;
use clap::Parser;
use risc0_zkvm::{Receipt, compute_image_id, serde::to_vec};
use sample_guest_common::IterReq;
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

    let client = ProvingClient::from_parts(args.endpoint, api_key, risc0_zkvm::VERSION).unwrap();

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

    // Execute STARK workflow
    let (_session_uuid, _receipt_id) =
        stark_workflow(&client, image.clone(), input, vec![], args.exec_only).await?;

    // return if exec only and success
    if args.exec_only {
        return Ok(());
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
