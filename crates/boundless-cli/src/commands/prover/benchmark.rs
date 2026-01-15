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

use alloy::primitives::U256;
use anyhow::{bail, Context, Result};
use bonsai_sdk::non_blocking::Client as BonsaiClient;
use boundless_market::{contracts::RequestInputType, input::GuestEnv, storage::fetch_url};
use clap::Args;
use risc0_zkvm::compute_image_id;
use sqlx::{postgres::PgPool, postgres::PgPoolOptions};

use crate::config::{GlobalConfig, ProverConfig};
use crate::config_ext::ProverConfigExt;
use crate::display::{network_name_from_chain_id, DisplayManager};

/// Benchmark proof requests
#[derive(Args, Clone, Debug)]
pub struct ProverBenchmark {
    /// Proof request ids to benchmark.
    #[arg(long, value_delimiter = ',', required = true)]
    pub request_ids: Vec<U256>,

    /// Prover configuration options
    #[clap(flatten, next_help_heading = "Prover")]
    pub prover_config: ProverConfig,
}
pub const RECOMMENDED_PEAK_PROVE_KHZ_FACTOR: f64 = 0.75;

#[derive(Debug, Clone)]
pub struct BenchmarkResult {
    pub worst_khz: f64,
    pub worst_recommended_khz: f64,
}

impl ProverBenchmark {
    /// Run the benchmark command
    pub async fn run(&self, global_config: &GlobalConfig) -> Result<BenchmarkResult> {
        let prover_config = self.prover_config.clone().load_and_validate()?;
        let client = prover_config.client_builder(global_config.tx_timeout)?.build().await?;
        let network_name = network_name_from_chain_id(client.deployment.market_chain_id);
        let display = DisplayManager::with_network(network_name);

        display.header("Benchmarking Proof Requests");
        display.item_colored("Total requests", self.request_ids.len(), "cyan");

        if prover_config.proving_backend.use_default_prover {
            bail!("benchmark command does not support using the default prover");
        }

        display.status("Status", "Configuring prover backend", "yellow");
        prover_config.proving_backend.configure_proving_backend_with_health_check().await?;
        let prover = BonsaiClient::from_env(risc0_zkvm::VERSION)?;

        // Track performance metrics across all runs
        let mut worst_khz = f64::MAX;
        let mut worst_recommended_khz = 0.0;
        let mut worst_time = 0.0;
        let mut worst_cycles = 0.0;
        let mut worst_request_id = U256::ZERO;

        // Check if we can connect to PostgreSQL using environment variables
        let pg_pool = match create_pg_pool().await {
            Ok(pool) => {
                display.item_colored("Database", "Connected to PostgreSQL", "green");
                Some(pool)
            }
            Err(e) => {
                tracing::debug!("Failed to connect to PostgreSQL database: {}", e);
                None
            }
        };

        display.separator();
        for (idx, request_id) in self.request_ids.iter().enumerate() {
            display.step(idx + 1, self.request_ids.len(), &format!("Request {:#x}", request_id));

            display.status("Status", "Fetching request details", "yellow");
            let (request, _signature) = client.fetch_proof_request(*request_id, None, None).await?;

            tracing::debug!("Fetched request 0x{:x}", request_id);
            tracing::debug!("Image URL: {}", request.imageUrl);

            display.status("Status", "Fetching program and input", "yellow");
            let elf = fetch_url(&request.imageUrl).await?;

            tracing::debug!("Processing input");
            let input = match request.input.inputType {
                RequestInputType::Inline => GuestEnv::decode(&request.input.data)?.stdin,
                RequestInputType::Url => {
                    let input_url = std::str::from_utf8(&request.input.data)
                        .context("Input URL is not valid UTF-8")?;
                    tracing::debug!("Fetching input from {}", input_url);
                    GuestEnv::decode(&fetch_url(input_url).await?)?.stdin
                }
                _ => bail!("Unsupported input type"),
            };

            display.status("Status", "Uploading to prover", "yellow");
            let image_id = compute_image_id(&elf)?.to_string();
            prover.upload_img(&image_id, elf).await.unwrap();
            tracing::debug!("Uploaded ELF to {}", image_id);

            let input_id =
                prover.upload_input(input).await.context("Failed to upload set-builder input")?;
            tracing::debug!("Uploaded input to {}", input_id);

            let assumptions = vec![];
            let start_time = std::time::Instant::now();

            display.status("Status", "Generating proof (this may take a while)", "yellow");
            let proof_id =
                prover.create_session(image_id, input_id, assumptions.clone(), false).await?;
            tracing::debug!("Created session {}", proof_id.uuid);

            let (stats, elapsed_time) = loop {
                let status = proof_id.status(&prover).await?;

                match status.status.as_ref() {
                    "RUNNING" => {
                        tokio::time::sleep(tokio::time::Duration::from_millis(1000)).await;
                        print!(".");
                        continue;
                    }
                    "SUCCEEDED" => {
                        let Some(stats) = status.stats else {
                            bail!("Bento failed to return proof stats in response");
                        };
                        break (stats, status.elapsed_time);
                    }
                    _ => {
                        let err_msg = status.error_msg.unwrap_or_default();
                        bail!("stark proving failed: {err_msg}");
                    }
                }
            };

            // Try to get effective KHz from PostgreSQL if available
            let (total_cycles, elapsed_secs) = if let Some(ref pool) = pg_pool {
                let total_cycles_query = r#"
                    SELECT (output->>'total_cycles')::FLOAT8
                    FROM tasks
                    WHERE task_id = 'init' AND job_id = $1::uuid
                "#;

                let elapsed_secs_query = r#"
                    SELECT EXTRACT(EPOCH FROM (MAX(updated_at) - MIN(started_at)))::FLOAT8
                    FROM tasks
                    WHERE job_id = $1::uuid
                "#;

                let cycles_result = sqlx::query_scalar::<_, f64>(total_cycles_query)
                    .bind(proof_id.uuid.clone())
                    .fetch_optional(pool)
                    .await;

                let elapsed_result = sqlx::query_scalar::<_, f64>(elapsed_secs_query)
                    .bind(proof_id.uuid.clone())
                    .fetch_optional(pool)
                    .await;

                match (cycles_result, elapsed_result) {
                    (Ok(Some(cycles)), Ok(Some(elapsed))) => {
                        tracing::debug!(
                            "Retrieved from PostgreSQL: {} cycles in {} seconds",
                            cycles,
                            elapsed
                        );
                        (cycles, elapsed)
                    }
                    _ => {
                        tracing::debug!("Failed to retrieve data from PostgreSQL, using client-side calculation");
                        let total_cycles: f64 = stats.total_cycles as f64;
                        let elapsed_secs = start_time.elapsed().as_secs_f64();
                        (total_cycles, elapsed_secs)
                    }
                }
            } else {
                tracing::debug!("No PostgreSQL data found for job, using client-side calculation.");
                let total_cycles: f64 = stats.total_cycles as f64;
                let elapsed_secs = start_time.elapsed().as_secs_f64();
                (total_cycles, elapsed_secs)
            };

            let effective_khz = total_cycles / elapsed_secs / 1000.0;

            display.item_colored("Status", "Proof completed", "green");
            display.item_colored("Cycles", format!("{:.0}", total_cycles), "cyan");
            display.item_colored("Time", format!("{:.2}s", elapsed_secs), "cyan");
            display.item_colored("Effective", format!("{:.2} KHz", effective_khz), "cyan");
            let recommended_khz = effective_khz * RECOMMENDED_PEAK_PROVE_KHZ_FACTOR;
            display.item_colored(
                &format!(
                    "Recommended `peak_prove_khz` ({:.0}% of effective)",
                    RECOMMENDED_PEAK_PROVE_KHZ_FACTOR * 100.0
                ),
                format!("{:.2} KHz", recommended_khz),
                "cyan",
            );

            if let Some(time) = elapsed_time {
                tracing::debug!("Server side time: {:?}", time);
            }

            // Track worst performance
            if effective_khz < worst_khz {
                worst_khz = effective_khz;
                worst_recommended_khz = recommended_khz;
                worst_time = elapsed_secs;
                worst_cycles = total_cycles;
                worst_request_id = *request_id;
            }
        }

        // Print summary
        display.separator();
        display.header("Benchmark Summary");
        display.item_colored("Total requests", self.request_ids.len(), "cyan");
        if !self.request_ids.is_empty() {
            display.note("Worst performance:");
            display.item_colored("Request ID", format!("{:#x}", worst_request_id), "yellow");
            display.item_colored("Effective", format!("{:.2} KHz", worst_khz), "yellow");
            display.item_colored(
                &format!(
                    "Recommended `peak_prove_khz` ({:.0}% of effective)",
                    RECOMMENDED_PEAK_PROVE_KHZ_FACTOR * 100.0
                ),
                format!("{:.2} KHz", worst_recommended_khz),
                "yellow",
            );
            display.item_colored("Cycles", format!("{:.0}", worst_cycles), "yellow");
            display.item_colored("Time", format!("{:.2}s", worst_time), "yellow");
        }

        Ok(BenchmarkResult { worst_khz, worst_recommended_khz })
    }
}

/// Create a PostgreSQL connection pool from environment variables
async fn create_pg_pool() -> Result<PgPool> {
    let database_url =
        std::env::var("DATABASE_URL").context("DATABASE_URL environment variable not set")?;

    let pool = PgPoolOptions::new()
        .max_connections(5)
        .connect(&database_url)
        .await
        .context("Failed to connect to PostgreSQL database")?;

    Ok(pool)
}
