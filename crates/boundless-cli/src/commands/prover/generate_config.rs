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

use std::path::{Path, PathBuf};

use alloy::primitives::utils::format_units;
use alloy::primitives::U256;
use alloy::providers::Provider;
use anyhow::{bail, Context, Result};
use boundless_market::indexer_client::IndexerClient;
use boundless_market::price_provider::{
    MarketPricing, MarketPricingConfigBuilder, PriceProvider, StandardPriceProvider,
};
use boundless_market::{
    Deployment, LARGE_REQUESTOR_LIST_THRESHOLD_KHZ, XL_REQUESTOR_LIST_THRESHOLD_KHZ,
};
use chrono::Utc;
use clap::Args;
use inquire::{Confirm, Select, Text};
use url::Url;

use super::benchmark::ProverBenchmark;
use crate::commands::prover::benchmark::BenchmarkResult;
use crate::config::{GlobalConfig, ProverConfig, ProvingBackendConfig};
use crate::config_file::Config;
use crate::display::{obscure_url, DisplayManager};

// Priority requestor list URLs
const PRIORITY_REQUESTOR_LIST_STANDARD: &str =
    "https://requestors.boundless.network/boundless-recommended-priority-list.standard.json";
const PRIORITY_REQUESTOR_LIST_LARGE: &str =
    "https://requestors.boundless.network/boundless-recommended-priority-list.large.json";
const PRIORITY_REQUESTOR_LIST_XL: &str =
    "https://requestors.boundless.network/boundless-recommended-priority-list.xl.json";

// Default minimum price per mega-cycle in collateral token (ZKC) for fulfilling
// orders locked by other provers that exceeded their lock timeout
const DEFAULT_MIN_MCYCLE_PRICE_COLLATERAL_TOKEN: &str = "0.0005";

mod selection_strings {
    // Benchmark performance options
    pub const BENCHMARK_RUN_SUITE: &str =
        "Run the Boundless benchmark suite (requires a Bento instance running)";
    pub const BENCHMARK_MANUAL_ENTRY: &str = "Manually set peak performance (in kHz)";

    // File handling strategy options
    pub const FILE_MODIFY_EXISTING: &str = "Modify existing";
    pub const FILE_GENERATE_NEW: &str = "Generate new";
    pub const FILE_CANCEL: &str = "Cancel";
}
/// Generate optimized broker.toml and compose.yml configuration files
#[derive(Args, Clone, Debug)]
pub struct ProverGenerateConfig {
    /// Path to output broker.toml file
    #[clap(long, default_value = "./broker.toml")]
    pub broker_toml_file: PathBuf,

    /// Path to output compose.yml file
    #[clap(long, default_value = "./compose.yml")]
    pub compose_yml_file: PathBuf,

    /// Skip creating backups of existing files
    #[clap(long)]
    pub skip_backup: bool,
}

#[derive(Debug)]
#[allow(dead_code)]
struct WizardConfig {
    num_threads: usize,
    num_gpus: usize,
    max_exec_agents: usize,
    max_concurrent_preflights: usize,
    max_concurrent_proofs: usize,
    peak_prove_khz: f64,
    segment_size: u32,
    priority_requestor_lists: Vec<String>,
    max_collateral: String,
    min_mcycle_price: String,
    min_mcycle_price_collateral_token: String,
}

#[derive(Debug, Clone, Copy)]
enum FileHandlingStrategy {
    ModifyExisting,
    GenerateNew,
}

impl ProverGenerateConfig {
    /// Run the generate-config command
    pub async fn run(&self, global_config: &GlobalConfig) -> Result<()> {
        let display = DisplayManager::new();

        display.header("Boundless Prover Configuration Wizard");
        display.note("This wizard helps you create Broker and Bento configuration files,");
        display.note("customized for your prover setup, that allow you to compete in the");
        display.note("market and earn rewards.");
        display.separator();
        display.info("For the best experience, ensure you have setup your system with the correct dependencies before continuing.");
        display.info("See https://docs.boundless.network/provers/quick-start#install-dependencies for more information.");

        // Check file handling strategy
        let broker_strategy =
            self.ask_file_handling_strategy(&self.broker_toml_file, "broker.toml", &display)?;
        let compose_strategy =
            self.ask_file_handling_strategy(&self.compose_yml_file, "compose.yml", &display)?;

        display.separator();

        // Run wizard to collect configuration
        let config = self.run_wizard(&display, global_config).await?;

        display.separator();
        display.header("Generating Configuration Files");

        // Backup and generate broker.toml
        if let Some(backup_path) = self.backup_file(&self.broker_toml_file)? {
            display.item_colored("Backup saved", backup_path.display(), "cyan");
        }
        self.generate_broker_toml(&config, broker_strategy, &display)?;
        display.item_colored("Created", self.broker_toml_file.display(), "green");

        // Backup and generate compose.yml
        if let Some(backup_path) = self.backup_file(&self.compose_yml_file)? {
            display.item_colored("Backup saved", backup_path.display(), "cyan");
        }
        self.generate_compose_yml(&config, compose_strategy, &display)?;
        display.item_colored("Created", self.compose_yml_file.display(), "green");

        display.separator();
        self.show_success_message(&config, &display)?;

        Ok(())
    }

    async fn run_wizard(
        &self,
        display: &DisplayManager,
        global_config: &GlobalConfig,
    ) -> Result<WizardConfig> {
        // Step 1: Machine configuration
        display.step(1, 7, "Machine Configuration");

        let run_on_single_machine =
            Confirm::new("Do you plan to run your prover entirely on your current machine?")
                .with_default(true)
                .with_help_message("This wizard is optimized for single-machine setups")
                .prompt()
                .context("Failed to get user input")?;

        if !run_on_single_machine {
            display.note("⚠  This wizard is optimized for single-machine setups.");
            display.note("   Cluster setups may require additional manual configuration.");
            display.note(&format!(
                "   Please refer to our documentation: {}",
                "https://docs.boundless.network/provers/broker"
            ));

            let continue_anyway = Confirm::new("Continue with configuration anyway?")
                .with_default(false)
                .with_help_message("Generated config may need manual adjustments for clusters")
                .prompt()
                .context("Failed to get confirmation")?;

            if !continue_anyway {
                display.note("Configuration cancelled.");
                bail!("Configuration cancelled by user");
            }
        }

        let detected_threads = detect_cpu_threads()?;
        display.item_colored("Detected", format!("{} CPU threads", detected_threads), "cyan");

        let input = Text::new("How many CPU threads do you want to use?")
            .with_default(&detected_threads.to_string())
            .prompt()
            .context("Failed to get CPU thread count")?;
        let num_threads = input.parse::<usize>().context("Invalid number format")?;
        display.item_colored("Using", format!("{} CPU threads", num_threads), "green");

        // Step 2: GPU configuration
        display.separator();
        display.step(2, 7, "GPU Configuration");

        let num_gpus = match detect_gpus() {
            Ok(count) if count > 0 => {
                display.item_colored("Detected", format!("{} GPU(s)", count), "cyan");
                count
            }
            _ => {
                let input = Text::new("How many GPUs do you have?")
                    .with_default("1")
                    .with_help_message("Enter 0 if you don't have any GPUs")
                    .prompt()
                    .context("Failed to get GPU count")?;
                input.parse::<usize>().context("Invalid number format")?
            }
        };

        // Detect GPU memory and segment size
        let segment_size = if num_gpus > 0 {
            match detect_gpu_memory() {
                Ok(memory_mib) => {
                    let memory_gb = (memory_mib as f64) / 1024.0;
                    display.item_colored(
                        "Min GPU memory",
                        format!("{:.1} GB ({} MiB)", memory_gb, memory_mib),
                        "cyan",
                    );

                    let recommended_segment_size = gpu_memory_to_segment_size(memory_mib);
                    let memory_range = if memory_mib <= 8_192 {
                        "≤8"
                    } else if memory_mib <= 16_384 {
                        "≤16"
                    } else if memory_mib <= 20_480 {
                        "≤20"
                    } else {
                        ">20"
                    };
                    display.item_colored(
                        "Segment size (po2)",
                        format!("{} (for {}GB VRAM)", recommended_segment_size, memory_range),
                        "cyan",
                    );
                    display.note(
                        "See: https://docs.boundless.network/provers/performance-optimization",
                    );
                    display.note("");
                    display.note("To start/restart Bento with this segment size:");
                    display.note(&format!("  export SEGMENT_SIZE={}", recommended_segment_size));
                    display.note("  just bento down && just bento up");
                    display.note("");

                    recommended_segment_size
                }
                Err(e) => {
                    display.note(&format!("⚠  Could not detect GPU memory: {}", e));
                    display.note("   Using default segment size (21)");
                    21
                }
            }
        } else {
            display.note("⚠  No GPUs detected, using default segment size (21)");
            21
        };

        // Step 3: Performance Benchmarking
        display.separator();
        display.step(3, 7, "Performance Benchmarking");

        let peak_prove_khz =
            self.get_peak_performance(display, global_config, segment_size).await?.floor();
        display.item_colored(
            "Setting `peak_prove_khz` to",
            format!("{:.0} kHz", peak_prove_khz),
            "green",
        );

        // Step 4: Calculated Configuration
        display.separator();
        display.step(4, 7, "Calculated Configuration");

        display.note("The following values are calculated based on your hardware:");
        display.note("");

        let max_exec_agents =
            ((num_threads.saturating_sub(4).saturating_sub(num_gpus * 2)) / 2).min(20);
        display.note("  Formula: max_exec_agents =");
        display.note("    min(20, # Capped at 20 to avoid overloading connections");
        display.note("      (");
        display.note(&format!("        {} threads", num_threads));
        display.note("        - 1  # reserve for postgres");
        display.note("        - 1  # reserve for redis");
        display.note("        - 1  # reserve for minio");
        display.note(&format!("        - {} GPUs × 2  # reserve two threads per GPU", num_gpus));
        display.note("      )");
        display.note("      / 2  # 2 threads per exec agent");
        display.note("    )");
        display.item_colored("  Result", format!("{} exec agents", max_exec_agents), "cyan");
        display.note("");

        let max_concurrent_preflights = max_exec_agents.saturating_sub(2).max(1);
        display.note("  Formula: max_concurrent_preflights =");
        display.note("    (");
        display.note(&format!("      {} exec agents", max_exec_agents));
        display.note("      - 1  # reserve for proofs");
        display.note("      - 1  # reserve for mining");
        display.note("    )");
        display.item_colored(
            "  Result",
            format!("{} concurrent preflights", max_concurrent_preflights),
            "cyan",
        );
        display.note("");

        let max_concurrent_proofs = 1;
        display.note("  Formula: max_concurrent_proofs = 1 (fixed)");
        display.item_colored(
            "  Result",
            format!("{} concurrent proof", max_concurrent_proofs),
            "cyan",
        );

        // Step 5: Priority Requestor Lists
        display.separator();
        display.step(5, 7, "Priority Requestor Lists");

        display.note("Requestor priority lists specify proof requestors that the broker should");
        display
            .note("prioritize for proving. Requestors on these lists are considered more likely");
        display
            .note("to request useful work with profitable pricing, and thus are prioritized over");
        display.note("other requestors.");
        display.note("");
        display.note("Boundless Networks maintains recommended requestor lists:");
        display.note("  • Standard list: For all provers (general workloads)");
        display.note(&format!(
            "  • Large list: For provers >{:.0} kHz (includes high-cycle orders)",
            LARGE_REQUESTOR_LIST_THRESHOLD_KHZ
        ));
        display.note(&format!(
            "  • XL list: For provers >{:.0} kHz (includes very high-cycle orders)",
            XL_REQUESTOR_LIST_THRESHOLD_KHZ
        ));
        display.note("");

        // Determine recommended lists based on peak performance
        let priority_requestor_lists = if peak_prove_khz > XL_REQUESTOR_LIST_THRESHOLD_KHZ {
            display.note(&format!(
                "Given your cluster's peak performance of {:.0} kHz, we recommend enabling the standard, large, and XL lists.",
                peak_prove_khz
            ));
            vec![
                PRIORITY_REQUESTOR_LIST_STANDARD.to_string(),
                PRIORITY_REQUESTOR_LIST_LARGE.to_string(),
                PRIORITY_REQUESTOR_LIST_XL.to_string(),
            ]
        } else if peak_prove_khz > LARGE_REQUESTOR_LIST_THRESHOLD_KHZ {
            display.note(&format!(
                "Given your cluster's peak performance of {:.0} kHz, we recommend enabling the standard and large lists.",
                peak_prove_khz
            ));
            vec![
                PRIORITY_REQUESTOR_LIST_STANDARD.to_string(),
                PRIORITY_REQUESTOR_LIST_LARGE.to_string(),
            ]
        } else {
            display.note(&format!(
                "Given your cluster's peak performance of {:.0} kHz, we recommend enabling the standard list.",
                peak_prove_khz
            ));
            vec![PRIORITY_REQUESTOR_LIST_STANDARD.to_string()]
        };

        display.note("");

        for list in &priority_requestor_lists {
            display.item_colored("  List", list, "cyan");
        }

        // Step 6: Collateral Configuration
        display.separator();
        display.step(6, 7, "Collateral Configuration");

        let recommended_collateral = match priority_requestor_lists.len() {
            1 => "50",
            2 => "200",
            _ => "500",
        };

        display.note(&format!(
            "We recommend a max collateral of {} ZKC for your configuration.",
            recommended_collateral
        ));
        display.note("  • 50 ZKC: Recommended for the standard requestor list");
        display.note("    (lower risk)");
        display.note("  • 200 ZKC: Recommended for standard + large lists");
        display.note("    (large orders, higher rewards, higher risk)");
        display.note("  • 500 ZKC: Recommended for standard + large + XL lists");
        display.note("    (largest orders, highest rewards, highest risk)");
        display.note("");
        display
            .note("Higher collateral enables higher-reward orders but increases slashing risks.");

        let max_collateral = Text::new("Max collateral (ZKC):")
            .with_default(recommended_collateral)
            .with_help_message("Press Enter to use recommended value")
            .prompt()
            .context("Failed to get max collateral")?;

        display.item_colored("Max collateral", format!("{} ZKC", max_collateral), "green");

        // Step 7: Pricing Configuration
        display.separator();
        display.step(7, 7, "Pricing Configuration");

        display.note("Analyzing recent market prices to determine competitive pricing...");
        display.note("");

        // Get RPC URL for market query
        let rpc_url = self.get_or_prompt_rpc_url(display)?;

        // Validate chain ID to ensure it's Base Mainnet
        display.status("Status", "Validating RPC connection", "yellow");
        let temp_provider = alloy::providers::ProviderBuilder::new()
            .connect(rpc_url.as_ref())
            .await
            .context("Failed to connect to RPC provider")?;

        let chain_id = temp_provider
            .get_chain_id()
            .await
            .context("Failed to query chain ID from RPC provider")?;

        if chain_id != 8453 {
            display.note(&format!("⚠  Detected Chain ID: {}", chain_id));
            display.note(
                "    Market pricing analyis requires a Base Mainnet RPC URL (Chain ID: 8453)",
            );
            let continue_anyway = Confirm::new("Continue with this RPC anyway?")
                .with_default(false)
                .prompt()
                .context("Failed to get confirmation")?;
            if !continue_anyway {
                bail!("Incorrect chain detected");
            }
        }

        let deployment = Deployment::from_chain_id(chain_id)
            .context("Failed to get deployment configuration for chain ID")?;
        let indexer_url =
            deployment.clone().indexer_url.context("No indexer URL found for deployment")?;
        let price_provider = StandardPriceProvider::new(
            IndexerClient::new(
                Url::parse(indexer_url.as_ref()).context("Failed to parse indexer URL")?,
            )
            .context("Failed to create indexer client")?,
        )
        .with_fallback(MarketPricing::new(
            Url::parse(rpc_url.as_ref()).context("Failed to parse RPC URL")?,
            MarketPricingConfigBuilder::default()
                .deployment(deployment)
                .build()
                .context("Failed to build market pricing config")?,
        ));

        // Query market pricing with fallback to defaults
        let market_pricing = match price_provider.price_percentiles().await {
            Ok(price_percentiles) => {
                display.item_colored(
                    "Median price",
                    format!(
                        "{} ETH/Mcycle ({} Gwei/Mcycle, {} wei/cycle)",
                        format_units(price_percentiles.p50 * U256::from(1_000_000), "ether")?,
                        format_units(price_percentiles.p50 * U256::from(1_000_000), "gwei")?,
                        format_units(price_percentiles.p50, "wei")?
                    ),
                    "cyan",
                );
                display.item_colored(
                    "25th percentile",
                    format!(
                        "{} ETH/Mcycle ({} Gwei/Mcycle, {} wei/cycle)",
                        format_units(price_percentiles.p25 * U256::from(1_000_000), "ether")?,
                        format_units(price_percentiles.p25 * U256::from(1_000_000), "gwei")?,
                        format_units(price_percentiles.p25, "wei")?
                    ),
                    "cyan",
                );
                Some(price_percentiles)
            }
            Err(e) => {
                display.note(&format!("⚠  Failed to query market prices: {}", e));
                display.note("Falling back to default pricing");
                None
            }
        };

        // Prompt user to accept or override
        let min_mcycle_price = if let Some(pricing) = market_pricing {
            display.note("");
            display.note(&format!(
                "Recommended minimum price: {} ETH/Mcycle ({} Gwei/Mcycle, {} wei/cycle)",
                format_units(pricing.p25 * U256::from(1_000_000), "ether")?,
                format_units(pricing.p25 * U256::from(1_000_000), "gwei")?,
                format_units(pricing.p25, "wei")?
            ));
            display.note("");
            display
                .note("This value is computed based on recent market prices. It ensures you are");
            display
                .note("priced competitively such that you will be able to lock and fulfill orders");
            display.note("for ETH rewards in the market.");
            display.note("");
            display.note("Example: If set to 0.00000001 ETH/Mcycle, a 1000 Mcycle order must");
            display.note("         offer at least 0.00001 ETH for your broker to accept it.");
            display.note("");

            Text::new("Press Enter to accept or enter custom price:")
                .with_default(&format!("{:.10}", pricing.p25))
                .with_help_message("You can update this later in broker.toml")
                .prompt()
                .context("Failed to get price")?
        } else {
            // Fallback to manual entry if query failed
            Text::new("Minimum price per mcycle (ETH):")
                .with_default("0.00000001")
                .with_help_message("You can update this later in broker.toml")
                .prompt()
                .context("Failed to get price")?
        };

        // Collateral token pricing
        display.separator();
        display.note("");
        display.note("Collateral Token Pricing:");
        display.note("");
        display.note("When another prover fails to fulfill their locked order within the timeout,");
        display.note("they are slashed. A portion of their collateral (in ZKC) becomes available");
        display.note("as a reward for any prover who can fulfill that order in a 'proof race'.");
        display.note("");
        display.note("The setting below controls the minimum ZKC reward your broker will accept");
        display.note("to participate in these proof races.");
        display.note("");
        display.note("Example: If set to 0.0005 ZKC/Mcycle, a 1000 Mcycle slashed order must");
        display.note("         offer at least 0.5 ZKC reward for your broker to compete for it.");
        display.note("");
        display.note(&format!(
            "Default minimum price: {} ZKC/Mcycle",
            DEFAULT_MIN_MCYCLE_PRICE_COLLATERAL_TOKEN
        ));
        display.note("");

        let min_mcycle_price_collateral_token =
            Text::new("Minimum price per Mcycle (in ZKC collateral rewards):")
                .with_default(DEFAULT_MIN_MCYCLE_PRICE_COLLATERAL_TOKEN)
                .with_help_message("You can update this later in broker.toml")
                .prompt()
                .context("Failed to get collateral token price")?;

        display.item_colored(
            "Collateral price",
            format!("{} ZKC/Mcycle", min_mcycle_price_collateral_token),
            "green",
        );

        let price_f64 = min_mcycle_price.parse::<f64>().unwrap_or(0.0);
        display.item_colored(
            "Min price",
            format!(
                "{} ETH/Mcycle ({} Gwei/Mcycle, {:.0} wei/cycle)",
                min_mcycle_price,
                price_f64 * 1e9,
                price_f64 * 1e12
            ),
            "green",
        );

        Ok(WizardConfig {
            num_threads,
            num_gpus,
            max_exec_agents,
            max_concurrent_preflights,
            max_concurrent_proofs,
            peak_prove_khz,
            segment_size,
            priority_requestor_lists,
            max_collateral,
            min_mcycle_price,
            min_mcycle_price_collateral_token,
        })
    }

    async fn get_peak_performance(
        &self,
        display: &DisplayManager,
        global_config: &GlobalConfig,
        segment_size: u32,
    ) -> Result<f64> {
        display.note("Configuration requires an estimate of the peak performance of your proving");
        display.note("cluster.");
        display.note("");
        display.note("Boundless provides a benchmarking suite for estimating your cluster's");
        display.note("peak performance.");
        display.warning("The benchmark suite requires access to a running Bento proving cluster.");
        display.note("For information on how to run Bento, see:");
        display.note("https://docs.boundless.network/provers/quick-start#running-a-test-proof");
        display.note("");

        let choice = Select::new(
            "How would you like to set the peak performance?",
            vec![selection_strings::BENCHMARK_RUN_SUITE, selection_strings::BENCHMARK_MANUAL_ENTRY],
        )
        .prompt()
        .context("Failed to get benchmark choice")?;

        match choice {
            selection_strings::BENCHMARK_RUN_SUITE => {
                // Get RPC URL before running benchmark
                display.separator();
                display.status("Status", "Checking RPC configuration", "yellow");
                let rpc_url = self.get_or_prompt_rpc_url(display)?;

                // Try to detect Bento at localhost
                let default_bento_url = "http://localhost:8081";
                let bento_available = check_bento_health(default_bento_url).await.is_ok();

                if bento_available {
                    display.item_colored("Bento", "Detected at http://localhost:8081", "green");

                    let use_detected = Confirm::new("Use this Bento instance for benchmarking?")
                        .with_default(true)
                        .prompt()
                        .context("Failed to get confirmation")?;

                    if use_detected {
                        if let Some(khz) = self
                            .try_run_benchmark_and_display(
                                default_bento_url,
                                &rpc_url,
                                global_config,
                                display,
                                segment_size,
                            )
                            .await
                        {
                            return Ok(khz);
                        }
                    }
                }

                // If not detected or user chose not to use detected, ask for custom URL
                if !bento_available {
                    display.note("⚠  Bento not detected at http://localhost:8081");
                }

                let provide_url =
                    Confirm::new("Do you have a Bento instance running at a different URL?")
                        .with_default(false)
                        .prompt()
                        .context("Failed to get URL confirmation")?;

                if provide_url {
                    let bento_url = Text::new("What is your Bento URL?")
                        .with_help_message("e.g., http://your-server:8081")
                        .prompt()
                        .context("Failed to get Bento URL")?;

                    if check_bento_health(&bento_url).await.is_ok() {
                        if let Some(khz) = self
                            .try_run_benchmark_and_display(
                                &bento_url,
                                &rpc_url,
                                global_config,
                                display,
                                segment_size,
                            )
                            .await
                        {
                            return Ok(khz);
                        }
                    } else {
                        display.note(&format!("⚠  Could not connect to Bento at {}", bento_url));
                        display.note("Falling back to manual input...");
                    }
                }

                // Fall through to manual input
                let khz_str = Text::new("Peak performance (kHz):")
                    .with_default("100")
                    .with_help_message("You can update this later in broker.toml")
                    .prompt()
                    .context("Failed to get peak performance")?;

                khz_str.parse::<f64>().context("Invalid performance value")
            }
            selection_strings::BENCHMARK_MANUAL_ENTRY => {
                let khz_str = Text::new("Peak performance (kHz):")
                    .with_default("100")
                    .with_help_message("You can update this later in broker.toml")
                    .prompt()
                    .context("Failed to get peak performance")?;

                khz_str.parse::<f64>().context("Invalid performance value")
            }
            _ => unreachable!(),
        }
    }

    async fn try_run_benchmark_and_display(
        &self,
        bento_url: &str,
        rpc_url: &Url,
        global_config: &GlobalConfig,
        display: &DisplayManager,
        segment_size: u32,
    ) -> Option<f64> {
        display.separator();
        display.note(&format!(
            "Reminder: Ensure Bento is running with SEGMENT_SIZE={} for accurate results",
            segment_size
        ));
        display.separator();
        display.status("Status", "Running benchmark", "yellow");
        display.note("This may take several minutes...");

        match self.run_benchmark(bento_url, rpc_url, global_config).await {
            Ok(BenchmarkResult { worst_recommended_khz, .. }) => Some(worst_recommended_khz),
            Err(e) => {
                display.note(&format!("⚠  Benchmark failed: {}", e));
                display.note("Falling back to manual input...");
                None
            }
        }
    }

    async fn run_benchmark(
        &self,
        bento_url: &str,
        rpc_url: &Url,
        global_config: &GlobalConfig,
    ) -> Result<BenchmarkResult> {
        // Use the hardcoded test request ID for benchmarking
        // TODO: use a representative request ID for benchmarking. This is a OG order of ~500M cycles.
        let request_id = "0xc197ebe12c7bcf1d9f3b415342bdbc795425335c01379fa6"
            .parse::<U256>()
            .context("Failed to parse request ID")?;

        // Create the benchmark command with proper configuration
        let benchmark = ProverBenchmark {
            request_ids: vec![request_id],
            prover_config: ProverConfig {
                prover_rpc_url: Some(rpc_url.clone()),
                private_key: None,
                prover_address: None,
                deployment: None,
                proving_backend: ProvingBackendConfig {
                    bento_api_url: bento_url.to_string(),
                    bento_api_key: None,
                    use_default_prover: false,
                    skip_health_check: false,
                },
            },
        };

        // Execute the benchmark and return the worst KHz value
        benchmark.run(global_config).await
    }

    fn get_or_prompt_rpc_url(&self, display: &DisplayManager) -> Result<Url> {
        // Try to load existing prover configuration
        if let Ok(config) = Config::load() {
            if let Some(_prover_config) = config.prover {
                // Try to load the full ProverConfig with RPC URL from environment/secrets
                let full_config = ProverConfig {
                    prover_rpc_url: None,
                    private_key: None,
                    prover_address: None,
                    deployment: None,
                    proving_backend: ProvingBackendConfig {
                        bento_api_url: "http://localhost:8081".to_string(),
                        bento_api_key: None,
                        use_default_prover: false,
                        skip_health_check: true,
                    },
                };

                if let Ok(loaded_config) = full_config.load_from_files() {
                    if let Some(rpc_url) = loaded_config.prover_rpc_url {
                        display.item_colored("RPC URL", obscure_url(rpc_url.as_ref()), "green");
                        return Ok(rpc_url);
                    }
                }
            }
        }

        // Check environment variable
        if let Ok(rpc_url) = std::env::var("PROVER_RPC_URL") {
            let url =
                rpc_url.parse::<Url>().context("Invalid PROVER_RPC_URL environment variable")?;
            display.item_colored("RPC URL", obscure_url(url.as_ref()), "green");
            return Ok(url);
        }

        // No RPC URL found, prompt user
        display.note("⚠  No RPC URL configured for prover");
        display.note("An RPC URL is required to fetch benchmark request data from the");
        display.note("blockchain.");
        display.note("");

        let rpc_url =
            Text::new("Enter Base Mainnet RPC URL:").prompt().context("Failed to get RPC URL")?;

        let url = rpc_url.parse::<Url>().context("Invalid RPC URL format")?;

        display.item_colored("RPC URL", obscure_url(url.as_ref()), "green");
        Ok(url)
    }

    fn backup_file(&self, file_path: &Path) -> Result<Option<PathBuf>> {
        // Skip if backup flag is set or file doesn't exist
        if self.skip_backup || !file_path.exists() {
            return Ok(None);
        }

        // Create backup directory
        let home = dirs::home_dir().context("Failed to get home directory")?;
        let backup_dir = home.join(".boundless").join("backups");
        std::fs::create_dir_all(&backup_dir).context("Failed to create backup directory")?;

        // Create timestamped backup filename
        let timestamp = Utc::now().format("%Y%m%d_%H%M%S");
        let filename = file_path.file_name().context("Invalid file path")?.to_string_lossy();
        let backup_filename = format!("{}.{}.bak", filename, timestamp);
        let backup_path = backup_dir.join(backup_filename);

        // Copy file to backup location
        std::fs::copy(file_path, &backup_path)
            .with_context(|| format!("Failed to create backup at {}", backup_path.display()))?;

        Ok(Some(backup_path))
    }

    fn strip_tagged_section(content: &str, opening_tag: &str, closing_tag: &str) -> String {
        if let Some(start) = content.find(opening_tag) {
            if let Some(end) = content[start..].find(closing_tag) {
                let mut section_end = start + end + closing_tag.len();

                // Also skip the newline after the closing tag if present
                if section_end < content.len() && content.as_bytes()[section_end] == b'\n' {
                    section_end += 1;
                }

                let before = &content[..start];
                let after = &content[section_end..];

                format!("{}{}", before, after)
            } else {
                content.to_string()
            }
        } else {
            content.to_string()
        }
    }

    fn generate_broker_toml(
        &self,
        config: &WizardConfig,
        strategy: FileHandlingStrategy,
        display: &DisplayManager,
    ) -> Result<()> {
        // Load source (template or existing file)
        let source = match strategy {
            FileHandlingStrategy::ModifyExisting => std::fs::read_to_string(&self.broker_toml_file)
                .context("Failed to read existing broker.toml")?,
            FileHandlingStrategy::GenerateNew => {
                include_str!("../../../../../broker-template.toml").to_string()
            }
        };

        // Parse with toml_edit (preserves comments and formatting)
        let mut doc = source.parse::<toml_edit::DocumentMut>().context("Failed to parse TOML")?;

        // Create CLI wizard metadata section with pretty tags
        let metadata_section = format!(
            "### [CLI Wizard Metadata] #####\n\
             # Generated by boundless-cli v{} on {}\n\
             # Hardware: {} threads, {} GPU(s)\n\
             # Peak performance: {:.0} kHz\n\
             ### [End] ###\n\n",
            env!("CARGO_PKG_VERSION"),
            Utc::now().format("%Y-%m-%d"),
            config.num_threads,
            config.num_gpus,
            config.peak_prove_khz
        );

        // Get current prefix
        let current_prefix = doc
            .as_table()
            .decor()
            .prefix()
            .map(|s| s.as_str().unwrap_or("").to_string())
            .unwrap_or_default();

        // Strip disclaimer section (from template)
        let cleaned_prefix =
            Self::strip_tagged_section(&current_prefix, "### [Disclaimer] ###", "### [End] ###");

        // Strip any existing CLI wizard metadata (from previous runs)
        let cleaned_prefix = Self::strip_tagged_section(
            &cleaned_prefix,
            "### [CLI Wizard Metadata] #####",
            "### [End] ###",
        );

        // Add new metadata section
        doc.as_table_mut()
            .decor_mut()
            .set_prefix(format!("{}{}", metadata_section, cleaned_prefix));

        // Update market section
        if let Some(market) = doc.get_mut("market").and_then(|v| v.as_table_mut()) {
            // Update peak_prove_khz with calculation comment
            if let Some(item) = market.get_mut("peak_prove_khz") {
                let comment =
                    format!("\n# Calculated from benchmark: {:.2} kHz\n", config.peak_prove_khz);
                if let Some(value) = item.as_value_mut() {
                    value.decor_mut().set_prefix(comment);
                }
                *item = toml_edit::value(config.peak_prove_khz as i64);
            }

            // Update max_collateral
            if let Some(item) = market.get_mut("max_collateral") {
                *item = toml_edit::value(config.max_collateral.clone());
            }

            // Update max_concurrent_proofs with calculation comment
            if let Some(item) = market.get_mut("max_concurrent_proofs") {
                let comment = format!("\n# Set based on GPU count: {} GPU(s)\n", config.num_gpus);
                if let Some(value) = item.as_value_mut() {
                    value.decor_mut().set_prefix(comment);
                }
                *item = toml_edit::value(config.max_concurrent_proofs as i64);
            }

            // Update priority_requestor_lists
            if let Some(item) = market.get_mut("priority_requestor_lists") {
                let mut arr = toml_edit::Array::new();
                for list in &config.priority_requestor_lists {
                    arr.push(list.clone());
                }
                *item = toml_edit::value(arr);
            }

            // Update max_concurrent_preflights with calculation comment
            if let Some(item) = market.get_mut("max_concurrent_preflights") {
                let comment = format!(
                    "\n# Calculated:\n\
                     # max_concurrent_preflights = (\n\
                     #   (\n\
                     #     {} threads\n\
                     #     - 1  # reserve for postgres\n\
                     #     - 1  # reserve for redis\n\
                     #     - 1  # reserve for minio\n\
                     #     - {} GPUs × 2  # reserve two threads per GPU\n\
                     #   )\n\
                     #   / 2  # 2 threads per exec agent\n\
                     #   - 1  # reserve for proofs\n\
                     #   - 1  # reserve for mining\n\
                     # )\n\
                     # = {}\n",
                    config.num_threads, config.num_gpus, config.max_concurrent_preflights
                );
                if let Some(value) = item.as_value_mut() {
                    value.decor_mut().set_prefix(comment);
                }
                *item = toml_edit::value(config.max_concurrent_preflights as i64);
            }

            // Update min_mcycle_price
            if let Some(item) = market.get_mut("min_mcycle_price") {
                let should_update = match strategy {
                    FileHandlingStrategy::ModifyExisting => {
                        // Get existing price and compare with recommended price
                        let existing_price_str = item.as_str().unwrap_or("0");
                        let existing_price = existing_price_str.parse::<f64>().unwrap_or(0.0);
                        let recommended_price =
                            config.min_mcycle_price.parse::<f64>().unwrap_or(0.0);

                        if existing_price <= recommended_price && existing_price > 0.0 {
                            // Existing price is already competitive, don't raise it
                            display.note("");
                            display.note(&format!(
                                "Your min_mcycle_price is already priced competitively at {} ETH/Mcycle. Not modifying.",
                                existing_price_str
                            ));
                            display.note("");
                            false
                        } else {
                            // Recommended price is lower (more competitive), update it
                            true
                        }
                    }
                    FileHandlingStrategy::GenerateNew => true,
                };

                if should_update {
                    *item = toml_edit::value(config.min_mcycle_price.clone());
                }
            }
        }

        // Write to file
        std::fs::write(&self.broker_toml_file, doc.to_string())
            .context("Failed to write broker.toml")?;

        Ok(())
    }

    fn count_existing_gpu_agents(&self, content: &str) -> usize {
        let mut count = 0;
        for line in content.lines() {
            if line.starts_with("  gpu_prove_agent") && line.ends_with(":") {
                count += 1;
            }
        }
        count
    }

    fn generate_compose_yml(
        &self,
        config: &WizardConfig,
        strategy: FileHandlingStrategy,
        display: &DisplayManager,
    ) -> Result<()> {
        // We use string manipulation instead of YAML parsing libraries because
        // compose.yml uses YAML anchors (&) and aliases (*) which are
        // not preserved by most Rust YAML libraries (serde_yaml, etc.).
        // This ensures all comments, formatting, and anchor definitions remain intact.

        // Load source (template or existing file)
        let mut content = match strategy {
            FileHandlingStrategy::ModifyExisting => std::fs::read_to_string(&self.compose_yml_file)
                .context("Failed to read existing compose.yml")?,
            FileHandlingStrategy::GenerateNew => {
                include_str!("../../../../../compose.yml").to_string()
            }
        };

        // Update exec_agent replicas
        content = self.update_exec_agent_replicas(content, config.max_exec_agents)?;

        // Update segment size
        content = self.update_segment_size(content, config.segment_size)?;

        // Handle GPU agents
        if matches!(strategy, FileHandlingStrategy::ModifyExisting) {
            let existing_gpu_count = self.count_existing_gpu_agents(&content);
            if existing_gpu_count > 1 {
                // File has already been modified with multiple GPUs
                display.note("");
                display.note("ℹ  The compose.yml GPU agents section has already been modified.");
                display.note(&format!(
                    "   Found {} GPU agents in the existing file.",
                    existing_gpu_count
                ));
                display.note("");
                display.note("   To change GPU configuration:");
                display.note("   1. Manually edit compose.yml");
                display.note("   2. Add/remove gpu_prove_agentN sections as needed");
                display.note("   3. Each agent should have a unique device_ids value");
                display.note("");
            } else if config.num_gpus > 1 {
                // Only add GPU agents if file has the default single GPU
                content = self.add_gpu_agents(content, config.num_gpus)?;
            }
        } else {
            // Generating new file - add GPU agents as needed
            if config.num_gpus > 1 {
                content = self.add_gpu_agents(content, config.num_gpus)?;
            }
        }

        // Write to file
        std::fs::write(&self.compose_yml_file, content).context("Failed to write compose.yml")?;

        Ok(())
    }

    fn update_exec_agent_replicas(&self, content: String, replicas: usize) -> Result<String> {
        let lines: Vec<&str> = content.lines().collect();
        let mut result: Vec<String> = Vec::new();
        let mut in_exec_agent = false;
        let mut in_deploy = false;

        for line in lines {
            let mut updated_line = line.to_string();

            // Track if we're in the exec_agent section
            if line.starts_with("  exec_agent:") {
                in_exec_agent = true;
                in_deploy = false;
            } else if in_exec_agent
                && line.starts_with("  ")
                && !line.starts_with("    ")
                && line.len() > 2
            {
                // We've hit another service at the same level, exit exec_agent section
                in_exec_agent = false;
                in_deploy = false;
            }

            // Track if we're in the deploy subsection
            if in_exec_agent && line.trim().starts_with("deploy:") {
                in_deploy = true;
            } else if in_deploy && !line.starts_with("    ") && !line.trim().is_empty() {
                // Exit deploy section if we hit a line at same or lower indentation
                in_deploy = false;
            }

            // Update replicas line if we're in the right section
            if in_exec_agent && in_deploy && line.trim().starts_with("replicas:") {
                let indent = line.chars().take_while(|c| c.is_whitespace()).collect::<String>();
                updated_line = format!("{}replicas: {}", indent, replicas);
            }

            result.push(updated_line);
        }

        Ok(result.join("\n"))
    }

    fn update_segment_size(&self, content: String, segment_size: u32) -> Result<String> {
        // Replace ${SEGMENT_SIZE:-21} with ${SEGMENT_SIZE:-<detected>}
        let pattern = "${SEGMENT_SIZE:-21}";
        let replacement = format!("${{SEGMENT_SIZE:-{}}}", segment_size);
        Ok(content.replace(pattern, &replacement))
    }

    fn add_gpu_agents(&self, content: String, num_gpus: usize) -> Result<String> {
        let lines: Vec<&str> = content.lines().collect();
        let mut result: Vec<String> = Vec::new();

        // Find gpu_prove_agent0 section boundaries
        let mut gpu_agent_start = None;
        let mut gpu_agent_end = None;

        for (i, line) in lines.iter().enumerate() {
            if line.starts_with("  gpu_prove_agent0:") {
                gpu_agent_start = Some(i);
            } else if gpu_agent_start.is_some() && gpu_agent_end.is_none() {
                // Look for next service at same indentation level (2 spaces, followed by a letter)
                if line.starts_with("  ")
                    && !line.starts_with("    ")
                    && line.len() > 2
                    && line.chars().nth(2).unwrap().is_alphabetic()
                {
                    gpu_agent_end = Some(i);
                    break;
                }
            }
        }

        let start =
            gpu_agent_start.context("Could not find gpu_prove_agent0 section in compose.yml")?;
        let end = gpu_agent_end.unwrap_or(lines.len());

        // Extract the gpu_prove_agent0 section
        let gpu_agent_lines: Vec<&str> = lines[start..end].to_vec();

        // Build result: everything up to and including gpu_prove_agent0
        for line in &lines[..end] {
            result.push(line.to_string());
        }

        // Add additional GPU agents
        for i in 1..num_gpus {
            result.push(String::new()); // Empty line between services

            for line in &gpu_agent_lines {
                let mut new_line = line.to_string();
                // Replace service name
                if new_line.contains("gpu_prove_agent0") {
                    new_line =
                        new_line.replace("gpu_prove_agent0", &format!("gpu_prove_agent{}", i));
                }
                // Replace device_ids
                if new_line.contains(r#"device_ids: ["0"]"#) {
                    new_line = new_line
                        .replace(r#"device_ids: ["0"]"#, &format!(r#"device_ids: ["{}"]"#, i));
                }
                result.push(new_line);
            }
        }

        // Add remaining content
        for line in &lines[end..] {
            result.push(line.to_string());
        }

        Ok(result.join("\n"))
    }

    fn ask_file_handling_strategy(
        &self,
        file_path: &Path,
        file_type: &str,
        display: &DisplayManager,
    ) -> Result<FileHandlingStrategy> {
        if !file_path.exists() {
            return Ok(FileHandlingStrategy::GenerateNew);
        }

        display.item_colored(
            "Found",
            format!("existing {} at {}", file_type, file_path.display()),
            "yellow",
        );

        let options = vec![
            selection_strings::FILE_MODIFY_EXISTING,
            selection_strings::FILE_GENERATE_NEW,
            selection_strings::FILE_CANCEL,
        ];

        let choice =
            Select::new(&format!("What would you like to do with {}?", file_type), options)
                .with_help_message("Modify preserves your customizations and comments")
                .prompt()
                .context("Failed to get file handling choice")?;

        match choice {
            selection_strings::FILE_MODIFY_EXISTING => Ok(FileHandlingStrategy::ModifyExisting),
            selection_strings::FILE_GENERATE_NEW => Ok(FileHandlingStrategy::GenerateNew),
            selection_strings::FILE_CANCEL => {
                bail!("Configuration cancelled by user");
            }
            _ => unreachable!(),
        }
    }

    fn show_success_message(&self, config: &WizardConfig, display: &DisplayManager) -> Result<()> {
        display.header("✓ Configuration Complete!");

        display.note("Generated files:");
        display.item_colored("  Broker config", self.broker_toml_file.display(), "green");
        display.item_colored("  Compose config", self.compose_yml_file.display(), "green");

        display.separator();
        display.note("Next steps:");
        display.note("1. Set the following environment variables:");

        // Check if env vars are set
        let private_key_set =
            std::env::var("PROVER_PRIVATE_KEY").or_else(|_| std::env::var("PRIVATE_KEY")).is_ok();
        let prover_rpc_url_set = std::env::var("PROVER_RPC_URL").is_ok();
        let legacy_rpc_url_set = std::env::var("RPC_URL").is_ok();
        let reward_address_set = std::env::var("REWARD_ADDRESS").is_ok();

        if private_key_set {
            display.item_colored("   PROVER_PRIVATE_KEY", "✓ Already set", "green");
        } else {
            display.note("   export PROVER_PRIVATE_KEY=<your_private_key>");
        }

        match (prover_rpc_url_set, legacy_rpc_url_set) {
            (true, _) => {
                display.item_colored("   PROVER_RPC_URL", "✓ Already set", "green");
            }
            (false, true) => {
                display.item_colored(
                    "   RPC_URL (legacy)",
                    "✓ Already set (fallback in use)",
                    "yellow",
                );
                display.note("   # Preferred: export PROVER_RPC_URL=<your_rpc_url>");
            }
            (false, false) => {
                display.note("   export PROVER_RPC_URL=<your_rpc_url>");
            }
        }

        if reward_address_set {
            display.item_colored("   REWARD_ADDRESS", "✓ Already set", "green");
        } else {
            display.warning("   REWARD_ADDRESS env variable is not set.");
            display
                .note("   This is required in order to receive ZKC mining rewards for your work.");
            display.note("   (This does not effect the ETH market fees you receive from");
            display.note("    fulfilling proving requests.)");
            display.note("   Learn more: https://docs.boundless.network/zkc/mining/overview");
            display.note("");
            display.note("   Option 1: export REWARD_ADDRESS=<reward_address>");
            display.note("   Option 2: Set POVW_LOG_ID in compose.yml to your reward address");
        }

        display.note("");
        display.note(&format!(
            "2. Ensure you have a minimum of {} ZKC collateral in your prover address:",
            config.max_collateral
        ));
        display.note("   boundless prover balance-collateral");

        display.note("");
        display.note("3. Start your prover:");
        display.note("   just prover up");

        display.note("");
        display.note("4. Monitor your prover:");
        display.note("   just prover logs");

        display.separator();
        display.note("For more information, visit:");
        display.note("https://docs.boundless.network/provers/broker");

        Ok(())
    }
}

// CPU thread detection
fn detect_cpu_threads() -> Result<usize> {
    Ok(num_cpus::get())
}

// GPU detection
fn detect_gpus() -> Result<usize> {
    // Try to detect NVIDIA GPUs using nvidia-smi
    let output = std::process::Command::new("nvidia-smi").arg("--list-gpus").output();

    match output {
        Ok(output) if output.status.success() => {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let count = stdout.lines().filter(|line| line.contains("GPU")).count();
            Ok(count)
        }
        _ => bail!("Could not detect GPUs automatically using `nvidia-smi --list-gpus`"),
    }
}

// GPU memory detection
fn detect_gpu_memory() -> Result<u32> {
    let output = std::process::Command::new("nvidia-smi")
        .arg("--query-gpu=memory.total")
        .arg("--format=csv,noheader,nounits")
        .output()
        .context("Failed to execute nvidia-smi")?;

    if !output.status.success() {
        bail!("nvidia-smi memory query failed");
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let min_memory = stdout
        .lines()
        .filter_map(|line| line.trim().parse::<u32>().ok())
        .min()
        .context("No GPU memory values found")?;

    Ok(min_memory)
}

// Map GPU memory to segment size
// Based on https://docs.boundless.network/provers/performance-optimization
// Convert MiB to GB for comparison: 1 GB = 1024 MiB
fn gpu_memory_to_segment_size(memory_mib: u32) -> u32 {
    match memory_mib {
        0..=8_192 => 18,       // <= 8GB
        8_193..=16_384 => 19,  // <= 16GB
        16_385..=20_480 => 20, // <= 20GB
        _ => 21,               // > 20GB (including 40GB+)
    }
}

// Bento health check
async fn check_bento_health(bento_url: &str) -> Result<()> {
    let url = Url::parse(bento_url).context("Invalid Bento URL")?;
    let health_url = url.join("health").context("Failed to construct hebalth check URL")?;

    reqwest::get(health_url.clone())
        .await
        .with_context(|| format!("Failed to connect to Bento at {}", health_url))?
        .error_for_status()
        .context("Bento health check returned error status")?;

    Ok(())
}
