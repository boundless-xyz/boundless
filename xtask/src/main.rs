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

use clap::{Parser, Subcommand};

mod bootstrap_blake3_groth16;
mod setup_blake3_groth16;
mod upload_blake3_groth16_artifacts;

#[derive(Parser)]
struct Cli {
    #[command(subcommand)]
    cmd: Commands,
}

#[derive(Subcommand)]
enum Commands {
    SetupBlake3Groth16(crate::setup_blake3_groth16::SetupBlake3Groth16),
    BootstrapBlake3Groth16(crate::bootstrap_blake3_groth16::BootstrapBlake3Groth16),
    UploadBlake3Groth16Artifacts(
        crate::upload_blake3_groth16_artifacts::UploadBlake3Groth16Artifacts,
    ),
}

impl Commands {
    async fn run(&self) {
        match self {
            Commands::SetupBlake3Groth16(cmd) => cmd.run(),
            Commands::BootstrapBlake3Groth16(cmd) => cmd.run().await,
            Commands::UploadBlake3Groth16Artifacts(cmd) => cmd.run().await,
        }
    }
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::filter::EnvFilter::from_default_env())
        .init();

    Cli::parse().cmd.run().await;
}
