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

use alloy::primitives::Address;
use anyhow::{bail, Context, Result};
use chrono::Utc;
use clap::Args;
use inquire::{Confirm, Select, Text};
use url::Url;

use crate::config::GlobalConfig;
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
}

#[derive(Debug)]
#[allow(dead_code)]
struct WizardConfig {
    num_threads: usize,
    num_gpus: usize,
    max_exec_agents: usize,
    max_concurrent_preflights: usize,
    max_concurrent_proofs: usize,
    reward_address: Address,
    peak_prove_khz: f64,
    priority_requestor_lists: Vec<String>,
    max_collateral: String,
    collateral_warning_threshold: String,
    collateral_error_threshold: String,
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
    pub async fn run(&self, _global_config: &GlobalConfig) -> Result<()> {
        let display = DisplayManager::new();

        display.header("Boundless Prover Configuration Wizard");
        display.note(
            "This wizard will help you create optimized configuration files for your prover setup.",
        );
        display.separator();

        // Check file handling strategy
        let broker_strategy =
            self.ask_file_handling_strategy(&self.broker_toml_file, "broker.toml", &display)?;
        let compose_strategy =
            self.ask_file_handling_strategy(&self.compose_yml_file, "compose.yml", &display)?;

        display.separator();

        // Run wizard to collect configuration
        let config = self.run_wizard(&display, broker_strategy).await?;

        display.separator();
        display.header("Generating Configuration Files");

        // Generate broker.toml
        display.status("Status", "Generating broker.toml", "yellow");
        self.generate_broker_toml(&config, broker_strategy)?;
        display.item_colored("Created", self.broker_toml_file.display(), "green");

        // Generate compose.yml
        display.status("Status", "Generating compose.yml", "yellow");
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

        // Calculate agent configuration
        let max_exec_agents = (num_threads.saturating_sub(4).saturating_sub(num_gpus * 2)) / 2;
        let max_concurrent_preflights = max_exec_agents.saturating_sub(2).max(1);
        let max_concurrent_proofs = 1;

        display.note("Calculated configuration:");
        display.item_colored("  Max exec agents", max_exec_agents, "cyan");
        display.item_colored("  Max concurrent preflights", max_concurrent_preflights, "cyan");
        display.item_colored("  Max concurrent proofs", max_concurrent_proofs, "cyan");

        // Step 3: Reward address
        display.separator();
        display.step(3, 7, "Reward Address");

        let reward_address = loop {
            let addr = Text::new("What is your REWARD_ADDRESS?")
                .with_help_message("Ethereum address where rewards will be sent")
                .prompt()
                .context("Failed to get reward address")?;

            match addr.parse::<Address>() {
                Ok(address) => {
                    display.item_colored("Address", format!("{:#x}", address), "green");
                    break address;
                }
                Err(_) => {
                    display.note("⚠  Invalid Ethereum address. Please try again.");
                }
            }
        };

        // Step 4: Benchmarking
        display.separator();
        display.step(4, 7, "Performance Benchmarking");

        let peak_prove_khz = self.get_peak_performance(display).await?;
        display.item_colored("Peak performance", format!("{:.2} kHz", peak_prove_khz), "green");

        // Step 5: Priority lists
        display.separator();
        display.step(5, 7, "Priority Requestor Lists");

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

        // Step 6: Collateral
        display.separator();
        display.step(6, 7, "Collateral Configuration");

        let recommended_collateral = if priority_requestor_lists.len() > 1 { "200" } else { "35" };

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

        let collateral_val: f64 = max_collateral.parse().context("Invalid collateral value")?;

        let collateral_warning_threshold = format!("{}", collateral_val * 2.0);
        let collateral_error_threshold = max_collateral.clone();

        display.item_colored("Max collateral", format!("{} ZKC", max_collateral), "green");
        display.item_colored(
            "Warning threshold",
            format!("{} ZKC", collateral_warning_threshold),
            "yellow",
        );
        display.item_colored(
            "Error threshold",
            format!("{} ZKC", collateral_error_threshold),
            "yellow",
        );

        // Step 7: Pricing (stub)
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
            reward_address,
            peak_prove_khz,
            priority_requestor_lists,
            max_collateral,
            collateral_warning_threshold,
            collateral_error_threshold,
            min_mcycle_price,
            min_mcycle_price_collateral_token,
        })
    }

    async fn get_peak_performance(&self, display: &DisplayManager) -> Result<f64> {
        // Try to detect Bento at localhost
        let default_bento_url = "http://localhost:8081";
        let bento_available = check_bento_health(default_bento_url).await.is_ok();

        if bento_available {
            display.item_colored("Bento", "Detected at http://localhost:8081", "green");

            let run_benchmark =
                Confirm::new("Do you want to run a benchmark to compute peak performance?")
                    .with_default(true)
                    .with_help_message("This will take a few minutes")
                    .prompt()
                    .context("Failed to get benchmark confirmation")?;

            if run_benchmark {
                display.status("Status", "Running benchmark", "yellow");
                display.note("This may take several minutes...");

                match self.run_benchmark(default_bento_url).await {
                    Ok(khz) => {
                        let adjusted_khz = khz * 0.75;
                        display.item_colored("Benchmark result", format!("{:.2} kHz", khz), "cyan");
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
        } else {
            display.note("⚠  Bento not detected at http://localhost:8081");

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
                    let run_benchmark = Confirm::new("Run benchmark on this Bento instance?")
                        .with_default(true)
                        .prompt()
                        .context("Failed to get benchmark confirmation")?;

                    if run_benchmark {
                        display.status("Status", "Running benchmark", "yellow");
                        match self.run_benchmark(&bento_url).await {
                            Ok(khz) => {
                                let adjusted_khz = khz * 0.75;
                                return Ok(adjusted_khz);
                            }
                            Err(e) => {
                                display.note(&format!("⚠  Benchmark failed: {}", e));
                            }
                        }
                    }
                } else {
                    display.note(&format!("⚠  Could not connect to Bento at {}", bento_url));
                }
            }
        }

        // Fall back to manual input
        let khz_str =
            Text::new("What is the estimated peak performance of your proving cluster (in kHz)?")
                .with_help_message("You can update this later in broker.toml")
                .with_default("100")
                .prompt()
                .context("Failed to get peak performance")?;

        khz_str.parse::<f64>().context("Invalid performance value")
    }

    async fn run_benchmark(&self, _bento_url: &str) -> Result<f64> {
        // TODO: Implement benchmark by invoking the benchmark command
        // For now, this is a stub that would need to:
        // 1. Set up environment variables for Bento
        // 2. Create a ProverBenchmark command with the specified order
        // 3. Execute it and extract the KHz result

        // Order to use: 0xc197ebe12c7bcf1d9f3b415342bdbc795425335cdbc3fef2
        bail!("Benchmark integration not yet implemented. Please enter peak performance manually.")
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

        // Prepend header to existing leading whitespace/comments
        let current_leading = doc
            .as_table()
            .decor()
            .prefix()
            .map(|s| s.as_str().unwrap_or("").to_string())
            .unwrap_or_default();
        doc.as_table_mut().decor_mut().set_prefix(format!("{}{}", header, current_leading));

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
                let calc_detail = format!(
                    "({} threads - 4 - {} GPUs * 2) / 2 - 2",
                    config.num_threads, config.num_gpus
                );
                let comment = format!(
                    "\n# Calculated: {} = {}\n",
                    calc_detail, config.max_concurrent_preflights
                );
                if let Some(value) = item.as_value_mut() {
                    value.decor_mut().set_prefix(comment);
                }
                *item = toml_edit::value(config.max_concurrent_preflights as i64);
            }

            // Add collateral thresholds if not present
            if market.get("collateral_balance_warn_threshold").is_none() {
                market.insert(
                    "collateral_balance_warn_threshold",
                    toml_edit::value(config.collateral_warning_threshold.clone()),
                );
            }
            if market.get("collateral_balance_error_threshold").is_none() {
                market.insert(
                    "collateral_balance_error_threshold",
                    toml_edit::value(config.collateral_error_threshold.clone()),
                );
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
        // Load source (template or existing file)
        let source = match strategy {
            FileHandlingStrategy::ModifyExisting => std::fs::read_to_string(&self.compose_yml_file)
                .context("Failed to read existing compose.yml")?,
            FileHandlingStrategy::GenerateNew => {
                include_str!("../../../../../compose.yml").to_string()
            }
        };

        // Parse as YAML
        let mut yaml: serde_yaml::Value =
            serde_yaml::from_str(&source).context("Failed to parse compose YAML")?;

        // Update exec_agent replicas
        if let Some(services) = yaml.get_mut("services").and_then(|v| v.as_mapping_mut()) {
            // Update exec_agent replicas
            if let Some(exec_agent) =
                services.get_mut("exec_agent").and_then(|v| v.as_mapping_mut())
            {
                if let Some(deploy) = exec_agent.get_mut("deploy").and_then(|v| v.as_mapping_mut())
                {
                    deploy.insert(
                        serde_yaml::Value::String("replicas".to_string()),
                        serde_yaml::Value::Number(config.max_exec_agents.into()),
                    );
                }
            }

            // Add additional GPU agents if needed
            if config.num_gpus > 1 {
                // Get the base gpu_agent0 configuration
                if let Some(gpu_agent0) = services.get("gpu_prove_agent0") {
                    let base_config = gpu_agent0.clone();

                    // Create additional GPU agents
                    for i in 1..config.num_gpus {
                        let agent_name = format!("gpu_prove_agent{}", i);
                        let mut agent_config = base_config.clone();

                        // Update device_ids
                        if let Some(agent_map) = agent_config.as_mapping_mut() {
                            if let Some(deploy) =
                                agent_map.get_mut("deploy").and_then(|v| v.as_mapping_mut())
                            {
                                if let Some(resources) =
                                    deploy.get_mut("resources").and_then(|v| v.as_mapping_mut())
                                {
                                    if let Some(reservations) = resources
                                        .get_mut("reservations")
                                        .and_then(|v| v.as_mapping_mut())
                                    {
                                        if let Some(devices) = reservations
                                            .get_mut("devices")
                                            .and_then(|v| v.as_sequence_mut())
                                        {
                                            if let Some(device) =
                                                devices.get_mut(0).and_then(|v| v.as_mapping_mut())
                                            {
                                                device.insert(
                                                    serde_yaml::Value::String(
                                                        "device_ids".to_string(),
                                                    ),
                                                    serde_yaml::Value::Sequence(vec![
                                                        serde_yaml::Value::String(i.to_string()),
                                                    ]),
                                                );
                                            }
                                        }
                                    }
                                }
                            }
                        }

                        services.insert(serde_yaml::Value::String(agent_name), agent_config);
                    }
                }
            }
        }

        // Write to file
        let yaml_string = serde_yaml::to_string(&yaml).context("Failed to serialize YAML")?;

        std::fs::write(&self.compose_yml_file, yaml_string)
            .context("Failed to write compose.yml")?;

        Ok(())
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
        let rpc_url_set = std::env::var("RPC_URL").is_ok();

        if private_key_set {
            display.item_colored("   PROVER_PRIVATE_KEY", "✓ Already set", "green");
        } else {
            display.note("   export PROVER_PRIVATE_KEY=<your_private_key>");
        }

        if rpc_url_set {
            display.item_colored("   RPC_URL", "✓ Already set", "green");
        } else {
            display.note("   export RPC_URL=<your_rpc_url>");
        }

        display.note(&format!("   export REWARD_ADDRESS={:#x}", config.reward_address));

        display.note("");
        display.note("2. Start your prover:");
        display.note("   docker-compose --profile broker up -d");

        display.note("");
        display.note("3. Monitor your prover:");
        display.note("   docker-compose logs -f broker");

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
