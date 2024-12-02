// Copyright 2024 RISC Zero, Inc.
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

use anyhow::{Context, Result};
use bonsai_sdk::non_blocking::Client as ProvingClient;
use clap::Parser;
use risc0_zkvm::compute_image_id;
use std::path::PathBuf;

// TODO: Delete the following hardcodes and the method_name include
// move all test vector guest images to their own guest in crates/guests
// and write tools to either generate inputs for this tool or
use risc0_zkvm::sha::Digest;
use serde::{Deserialize, Serialize};

/// Taken from crates/guests/
#[derive(Deserialize, Serialize)]
pub enum IterReq {
    Iter(u64),
    Composition(u64, Digest),
    DoubleComposition(u64, Digest, Digest),
}

const TEST_ELF: &[u8] = include_bytes!("../method_name");

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

    /// Optionally Create a SNARK proof
    #[clap(short, long, default_value_t = false, conflicts_with = "exec_only")]
    snarkify: bool,

    /// Run a execute only job, aka preflight
    ///
    /// Useful for capturing metrics on a STARK proof like cycles.
    #[clap(short, long, default_value_t = false, conflicts_with = "snarkify")]
    exec_only: bool,

    /// Bento HTTP API Endpoint
    #[clap(short = 't', long, default_value = "http://localhost:8081")]
    endpoint: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let args = Args::parse();

    let client =
        ProvingClient::from_parts(args.endpoint, String::new(), risc0_zkvm::VERSION).unwrap();

    let (image, input) = if let Some(elf_file) = args.elf_file {
        let image = std::fs::read(elf_file).context("Failed to read elf file from disk")?;
        let input = std::fs::read(
            args.input_file.expect("if --elf-file is supplied, supply a --input-file"),
        )?;

        (image, input)
    } else if let Some(iter_count) = args.iter_count {
        let input = IterReq::Iter(iter_count);
        let input = risc0_zkvm::serde::to_vec(&input).expect("Failed to r0 to_vec");
        let input = bytemuck::cast_slice(&input).to_vec();

        (TEST_ELF.to_vec(), input)
    } else {
        panic!("Invalid arg config, either elf_file or iter_count should be supplied");
    };

    let image_id = compute_image_id(&image).unwrap().to_string();

    client.upload_img(&image_id, image).await.context("Failed to upload image")?;

    std::fs::write("/tmp/input.bin", &input)?;
    let input_id = client.upload_input(input).await.context("Failed to upload_input")?;

    tracing::info!("image_id: {image_id} | input_id: {input_id}");

    let session = client
        .create_session(image_id, input_id, vec![], args.exec_only)
        .await
        .context("Failed to stark STARK proving")?;

    tracing::info!("STARK job_id: {}", session.uuid);

    loop {
        let res = session.status(&client).await.context("Failed to get STARK status")?;

        match res.status.as_ref() {
            "RUNNING" => {
                tracing::info!("STARK Job running....");
                tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;
                continue;
            }
            "SUCCEEDED" => {
                tracing::info!("Job done!");
                let _receipt = client
                    .receipt_download(&session)
                    .await
                    .context("Failed to download receipt")?;

                // let new_receipt_id =
                //     client.upload_receipt(receipt).await.context("Failed to upload receipt")?;
                // tracing::info!("downloaded and uploaded receipt, new id: {new_receipt_id}");
                break;
            }
            _ => {
                tracing::error!(
                    "Job failed: {} - {}",
                    session.uuid,
                    res.error_msg.as_ref().unwrap_or(&String::new())
                );
                break;
            }
        }
    }

    if args.snarkify {
        let snark_session =
            client.create_snark(session.uuid).await.context("Failed to create SNARK session")?;
        loop {
            let res = snark_session
                .status(&client)
                .await
                .context("Failed to get snark session status")?;
            match res.status.as_ref() {
                "RUNNING" => {
                    tracing::info!("SNARK Job running....");
                    tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
                    continue;
                }
                "SUCCEEDED" => {
                    tracing::info!("Job done!");

                    let _receipt = client
                        .download(&res.output.context("SNARK missing output URL")?)
                        .await
                        .context("Failed to download snark receipt")?;
                    break;
                }
                _ => {
                    tracing::error!(
                        "Job failed: {} - {}",
                        snark_session.uuid,
                        res.error_msg.as_ref().unwrap_or(&String::new())
                    );
                    break;
                }
            }
        }
    }

    Ok(())
}
