// Copyright 2025 RISC Zero, Inc.
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

use alloy::primitives::U256;
use anyhow::{bail, Context, Result};
use chrono::Utc;
use clap::Args;
use inquire::{Confirm, Select, Text};
use url::Url;

use super::benchmark::ProverBenchmark;
use crate::config::{GlobalConfig, ProverConfig, ProvingBackendConfig};
use crate::config_file::Config;
use crate::display::DisplayManager;

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
        display.note(
            "This wizard helps you create Broker and Bento configuration files, customized for your prover setup, that allow you to compete in the market and earn rewards.",
        );
        display.separator();

        // Check file handling strategy
        let broker_strategy =
            self.ask_file_handling_strategy(&self.broker_toml_file, "broker.toml", &display)?;
        let compose_strategy =
            self.ask_file_handling_strategy(&self.compose_yml_file, "compose.yml", &display)?;

        display.separator();

        // Run wizard to collect configuration
        let config = self.run_wizard(&display, broker_strategy, global_config).await?;

        display.separator();
        display.header("Generating Configuration Files");

        // Backup and generate broker.toml
        if let Some(backup_path) = self.backup_file(&self.broker_toml_file)? {
            display.item_colored(
                "Backup saved",
                backup_path.display(),
                "cyan",
            );
        }
        self.generate_broker_toml(&config, broker_strategy)?;
        display.item_colored("Created", self.broker_toml_file.display(), "green");

        // Backup and generate compose.yml
        if let Some(backup_path) = self.backup_file(&self.compose_yml_file)? {
            display.item_colored(
                "Backup saved",
                backup_path.display(),
                "cyan",
            );
        }
        self.generate_compose_yml(&config, compose_strategy)?;
        display.item_colored("Created", self.compose_yml_file.display(), "green");

        display.separator();
        self.show_success_message(&config, &display)?;

        Ok(())
    }

    fn parse_existing_broker_toml(&self) -> Result<Option<toml::Value>> {
        if !self.broker_toml_file.exists() {
            return Ok(None);
        }

        let content = std::fs::read_to_string(&self.broker_toml_file)
            .context("Failed to read existing broker.toml")?;
        let parsed: toml::Value =
            toml::from_str(&content).context("Failed to parse existing broker.toml")?;
        Ok(Some(parsed))
    }

    async fn run_wizard(
        &self,
        display: &DisplayManager,
        broker_strategy: FileHandlingStrategy,
        global_config: &GlobalConfig,
    ) -> Result<WizardConfig> {
        // Try to parse existing config if modifying
        let existing_config = match broker_strategy {
            FileHandlingStrategy::ModifyExisting => self.parse_existing_broker_toml()?,
            FileHandlingStrategy::GenerateNew => None,
        };

        if existing_config.is_some() {
            display.note("📄 Using values from existing broker.toml as defaults");
            display.separator();
        }
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

        let num_threads = detect_cpu_threads()?;
        display.item_colored("Detected", format!("{} CPU threads", num_threads), "cyan");

        let use_detected_threads = Confirm::new("Use this value?")
            .with_default(true)
            .prompt()
            .context("Failed to get confirmation")?;

        let num_threads = if !use_detected_threads {
            let input = Text::new("How many CPU threads?")
                .with_default(&num_threads.to_string())
                .prompt()
                .context("Failed to get CPU thread count")?;
            let override_threads = input.parse::<usize>().context("Invalid number format")?;
            display.item_colored("Using", format!("{} CPU threads", override_threads), "green");
            override_threads
        } else {
            num_threads
        };

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

        // Step 3: Calculated Configuration
        display.separator();
        display.step(3, 7, "Calculated Configuration");

        display.note("The following values are calculated based on your hardware:");
        display.note("");

        let max_exec_agents = (num_threads.saturating_sub(4).saturating_sub(num_gpus * 2)) / 2;
        display.note("  Formula: max_exec_agents =");
        display.note("    (");
        display.note(&format!("      {} threads", num_threads));
        display.note("      - 1  # reserve for postgres");
        display.note("      - 1  # reserve for redis");
        display.note("      - 1  # reserve for minio");
        display.note(&format!("      - {} GPUs × 2  # reserve two threads per GPU", num_gpus));
        display.note("    )");
        display.note("    / 2  # 2 threads per exec agent");
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

        // Step 4: Performance Benchmarking
        display.separator();
        display.step(4, 7, "Performance Benchmarking");

        let peak_prove_khz = self.get_peak_performance(display, global_config).await?;
        display.item_colored("Peak performance", format!("{:.2} kHz", peak_prove_khz), "green");

        // Step 5: Priority Requestor Lists
        display.separator();
        display.step(5, 7, "Priority Requestor Lists");

        display.note("Requestor priority lists specify proof requestors that the broker should");
        display.note("prioritize for proving. Requestors on these lists are considered more likely");
        display.note("to request useful work with profitable pricing, and thus are prioritized over");
        display.note("other requestors. Boundless Networks maintains a recommended list of requestors that");
        display.note("will be enabled by default.");
        display.note("");

        let priority_requestor_lists = if peak_prove_khz > 4000.0 {
            display.note("Your cluster is powerful enough for both standard and large requestors");
            vec![
                "https://requestors.boundless.network/boundless-recommended-priority-list.standard.json".to_string(),
                "https://requestors.boundless.network/boundless-recommended-priority-list.large.json".to_string(),
            ]
        } else {
            display.note("Your cluster will use the standard requestor list");
            vec![
                "https://requestors.boundless.network/boundless-recommended-priority-list.standard.json".to_string(),
            ]
        };

        for list in &priority_requestor_lists {
            display.item_colored("  List", list, "cyan");
        }

        // Step 6: Collateral Configuration
        display.separator();
        display.step(6, 7, "Collateral Configuration");

        let recommended_collateral = if priority_requestor_lists.len() > 1 { "200" } else { "50" };

        display.note(&format!(
            "We recommend a max collateral of {} ZKC for your configuration.",
            recommended_collateral
        ));
        display.note(
            "Setting higher collateral enables higher-reward orders with higher slashing risks.",
        );

        let max_collateral = Text::new("Max collateral (ZKC):")
            .with_default(recommended_collateral)
            .with_help_message("Press Enter to use recommended value")
            .prompt()
            .context("Failed to get max collateral")?;

        display.item_colored("Max collateral", format!("{} ZKC", max_collateral), "green");

        // Step 7: Pricing Configuration
        display.separator();
        display.step(7, 7, "Pricing Configuration");

        display.note("Using default pricing values from template.");
        display.note("(Custom pricing configuration will be available in a future update)");

        let min_mcycle_price = "0.0000005".to_string();
        let min_mcycle_price_collateral_token = "0.0001".to_string();

        Ok(WizardConfig {
            num_threads,
            num_gpus,
            max_exec_agents,
            max_concurrent_preflights,
            max_concurrent_proofs,
            peak_prove_khz,
            priority_requestor_lists,
            max_collateral,
            min_mcycle_price,
            min_mcycle_price_collateral_token,
        })
    }

    async fn get_peak_performance(&self, display: &DisplayManager, global_config: &GlobalConfig) -> Result<f64> {
        display.note("Configuration requires an estimate of the peak performance of your proving");
        display.note("cluster.");
        display.note("");

        let choice = Select::new(
            "How would you like to set the peak performance?",
            vec![
                "Run the Boundless benchmark suite",
                "Manually set peak performance (in kHz)",
            ],
        )
        .prompt()
        .context("Failed to get benchmark choice")?;

        match choice {
            "Run the Boundless benchmark suite" => {
                // Get RPC URL before running benchmark
                display.separator();
                display.status("Status", "Checking RPC configuration", "yellow");
                let rpc_url = self.get_or_prompt_rpc_url(display)?;

                // Try to detect Bento at localhost
                let default_bento_url = "http://localhost:8081";
                let bento_available = check_bento_health(default_bento_url).await.is_ok();

                if bento_available {
                    display.item_colored("Bento", "Detected at http://localhost:8081", "green");

                    let use_detected =
                        Confirm::new("Use this Bento instance for benchmarking?")
                            .with_default(true)
                            .prompt()
                            .context("Failed to get confirmation")?;

                    if use_detected {
                        display.status("Status", "Running benchmark", "yellow");
                        display.note("This may take several minutes...");

                        match self.run_benchmark(default_bento_url, &rpc_url, global_config).await {
                            Ok(khz) => {
                                let adjusted_khz = khz * 0.75;
                                display.item_colored(
                                    "Benchmark result",
                                    format!("{:.2} kHz", khz),
                                    "cyan",
                                );
                                display.item_colored(
                                    "Adjusted (75%)",
                                    format!("{:.2} kHz", adjusted_khz),
                                    "cyan",
                                );
                                return Ok(adjusted_khz);
                            }
                            Err(e) => {
                                display.note(&format!("⚠  Benchmark failed: {}", e));
                                display.note("Falling back to manual input...");
                            }
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
                        display.status("Status", "Running benchmark", "yellow");
                        display.note("This may take several minutes...");

                        match self.run_benchmark(&bento_url, &rpc_url, global_config).await {
                            Ok(khz) => {
                                let adjusted_khz = khz * 0.75;
                                display.item_colored(
                                    "Benchmark result",
                                    format!("{:.2} kHz", khz),
                                    "cyan",
                                );
                                display.item_colored(
                                    "Adjusted (75%)",
                                    format!("{:.2} kHz", adjusted_khz),
                                    "cyan",
                                );
                                return Ok(adjusted_khz);
                            }
                            Err(e) => {
                                display.note(&format!("⚠  Benchmark failed: {}", e));
                                display.note("Falling back to manual input...");
                            }
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
            "Manually set peak performance (in kHz)" => {
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

    async fn run_benchmark(&self, bento_url: &str, rpc_url: &Url, global_config: &GlobalConfig) -> Result<f64> {
        // Use the hardcoded test request ID for benchmarking
        let request_id = "0xc197ebe12c7bcf1d9f3b415342bdbc795425335cdbc3fef2"
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
                        display.item_colored("RPC URL", &rpc_url, "green");
                        return Ok(rpc_url);
                    }
                }
            }
        }

        // Check environment variable
        if let Ok(rpc_url) = std::env::var("PROVER_RPC_URL") {
            let url = rpc_url.parse::<Url>()
                .context("Invalid PROVER_RPC_URL environment variable")?;
            display.item_colored("RPC URL", &url, "green");
            return Ok(url);
        }

        // No RPC URL found, prompt user
        display.note("⚠  No RPC URL configured for prover");
        display.note("An RPC URL is required to fetch benchmark request data from the blockchain.");
        display.note("");

        let rpc_url = Text::new("Enter Base Mainnet RPC URL:")
            .with_help_message("e.g., https://mainnet.base.org or your Alchemy/Infura URL")
            .prompt()
            .context("Failed to get RPC URL")?;

        let url = rpc_url.parse::<Url>()
            .context("Invalid RPC URL format")?;

        display.item_colored("RPC URL", &url, "green");
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
        std::fs::create_dir_all(&backup_dir)
            .context("Failed to create backup directory")?;

        // Create timestamped backup filename
        let timestamp = Utc::now().format("%Y%m%d_%H%M%S");
        let filename = file_path
            .file_name()
            .context("Invalid file path")?
            .to_string_lossy();
        let backup_filename = format!("{}.{}.bak", filename, timestamp);
        let backup_path = backup_dir.join(backup_filename);

        // Copy file to backup location
        std::fs::copy(file_path, &backup_path)
            .with_context(|| format!("Failed to create backup at {}", backup_path.display()))?;

        Ok(Some(backup_path))
    }

    fn generate_broker_toml(
        &self,
        config: &WizardConfig,
        strategy: FileHandlingStrategy,
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

        // Add generation header
        let header = format!(
            "# Generated by boundless-cli v{} on {}\n\
             # Hardware: {} threads, {} GPU(s)\n\
             # Peak performance: {:.2} kHz\n\n",
            env!("CARGO_PKG_VERSION"),
            Utc::now().format("%Y-%m-%d"),
            config.num_threads,
            config.num_gpus,
            config.peak_prove_khz
        );

        // Handle prefix based on strategy
        match strategy {
            FileHandlingStrategy::GenerateNew => {
                // For new files, replace the template disclaimer with our generation header
                doc.as_table_mut().decor_mut().set_prefix(header);
            }
            FileHandlingStrategy::ModifyExisting => {
                // For existing files, preserve current prefix and prepend header
                let current_leading = doc
                    .as_table()
                    .decor()
                    .prefix()
                    .map(|s| s.as_str().unwrap_or("").to_string())
                    .unwrap_or_default();
                doc.as_table_mut()
                    .decor_mut()
                    .set_prefix(format!("{}{}", header, current_leading));
            }
        }

        // Update market section
        if let Some(market) = doc.get_mut("market").and_then(|v| v.as_table_mut()) {
            // Update peak_prove_khz with calculation comment
            if let Some(item) = market.get_mut("peak_prove_khz") {
                let comment =
                    format!("\n# Calculated from benchmark: {:.2} kHz\n", config.peak_prove_khz);
                if let Some(value) = item.as_value_mut() {
                    value.decor_mut().set_prefix(comment);
                }
                *item = toml_edit::value(config.peak_prove_khz);
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
        }

        // Write to file
        std::fs::write(&self.broker_toml_file, doc.to_string())
            .context("Failed to write broker.toml")?;

        Ok(())
    }

    fn generate_compose_yml(
        &self,
        config: &WizardConfig,
        strategy: FileHandlingStrategy,
    ) -> Result<()> {
        // We use string manipulation instead of YAML parsing libraries because
        // compose.yml heavily uses YAML anchors (&) and aliases (*) which are
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

        // Add additional GPU agents if needed
        if config.num_gpus > 1 {
            content = self.add_gpu_agents(content, config.num_gpus)?;
        }

        // Write to file
        std::fs::write(&self.compose_yml_file, content)
            .context("Failed to write compose.yml")?;

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
            } else if in_exec_agent && line.starts_with("  ") && !line.starts_with("    ") && line.len() > 2 {
                // We've hit another service at the same level, exit exec_agent section
                in_exec_agent = false;
                in_deploy = false;
            }

            // Track if we're in the deploy subsection
            if in_exec_agent && line.trim().starts_with("deploy:") {
                in_deploy = true;
            } else if in_deploy && !line.starts_with("    ") && line.trim().len() > 0 {
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
                if line.starts_with("  ") && !line.starts_with("    ") && line.len() > 2 && line.chars().nth(2).unwrap().is_alphabetic() {
                    gpu_agent_end = Some(i);
                    break;
                }
            }
        }

        let start = gpu_agent_start.context("Could not find gpu_prove_agent0 section in compose.yml")?;
        let end = gpu_agent_end.unwrap_or(lines.len());

        // Extract the gpu_prove_agent0 section
        let gpu_agent_lines: Vec<&str> = lines[start..end].to_vec();

        // Build result: everything up to and including gpu_prove_agent0
        for line in &lines[..end] {
            result.push(line.to_string());
        }

        // Add additional GPU agents
        for i in 1..num_gpus {
            result.push(String::new());  // Empty line between services

            for line in &gpu_agent_lines {
                let mut new_line = line.to_string();
                // Replace service name
                if new_line.contains("gpu_prove_agent0") {
                    new_line = new_line.replace("gpu_prove_agent0", &format!("gpu_prove_agent{}", i));
                }
                // Replace device_ids
                if new_line.contains(r#"device_ids: ["0"]"#) {
                    new_line = new_line.replace(r#"device_ids: ["0"]"#, &format!(r#"device_ids: ["{}"]"#, i));
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

        let options = vec!["Modify existing", "Generate new", "Cancel"];

        let choice =
            Select::new(&format!("What would you like to do with {}?", file_type), options)
                .with_help_message("Modify preserves your customizations and comments")
                .prompt()
                .context("Failed to get file handling choice")?;

        match choice {
            "Modify existing" => Ok(FileHandlingStrategy::ModifyExisting),
            "Generate new" => Ok(FileHandlingStrategy::GenerateNew),
            "Cancel" => {
                bail!("Configuration cancelled by user");
            }
            _ => unreachable!(),
        }
    }

    fn show_success_message(&self, _config: &WizardConfig, display: &DisplayManager) -> Result<()> {
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

        display.note("");
        display.note("2. Ensure you have enough collateral in your prover address:");
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

// Bento health check
async fn check_bento_health(bento_url: &str) -> Result<()> {
    let url = Url::parse(bento_url).context("Invalid Bento URL")?;
    let health_url = url.join("health").context("Failed to construct health check URL")?;

    reqwest::get(health_url.clone())
        .await
        .with_context(|| format!("Failed to connect to Bento at {}", health_url))?
        .error_for_status()
        .context("Bento health check returned error status")?;

    Ok(())
}
