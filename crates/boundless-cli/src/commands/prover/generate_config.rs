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

use super::benchmark::ProverBenchmark;
use crate::commands::prover::benchmark::BenchmarkResult;
use crate::config::{GlobalConfig, ProverConfig, ProvingBackendConfig};
use crate::config_file::Config;
use crate::display::{obscure_url, DisplayManager};
use crate::price_oracle_helper::{
    fetch_and_display_prices, format_amount_with_conversion, prompt_validated_amount,
    try_convert_to_usd, try_init_price_oracle,
};
use alloy::primitives::utils::format_units;
use alloy::primitives::U256;
use alloy::providers::Provider;
use anyhow::{bail, Context, Result};
use boundless_market::indexer_client::IndexerClient;
use boundless_market::price_oracle::Asset;
use boundless_market::price_provider::{
    MarketPricing, MarketPricingConfigBuilder, PriceProvider, StandardPriceProvider,
};
use boundless_market::{
    Deployment, LARGE_REQUESTOR_LIST_THRESHOLD_KHZ, SUPPORTED_CHAINS,
    XL_REQUESTOR_LIST_THRESHOLD_KHZ,
};
use broker::config::CHAIN_OVERRIDES_DIR;
use chrono::Utc;
use clap::Args;
use inquire::{Confirm, MultiSelect, Select, Text};
use std::path::{Path, PathBuf};
use url::Url;

// Priority requestor list URLs
const PRIORITY_REQUESTOR_LIST_STANDARD: &str =
    "https://requestors.boundless.network/boundless-recommended-priority-list.standard.json";
const PRIORITY_REQUESTOR_LIST_LARGE: &str =
    "https://requestors.boundless.network/boundless-recommended-priority-list.large.json";
const PRIORITY_REQUESTOR_LIST_XL: &str =
    "https://requestors.boundless.network/boundless-recommended-priority-list.xl.json";

// Keys under [market] that were renamed. Old names are silently dropped on load,
// which looks like the user's tuning is in effect when it isn't.
// The wizard surfaces these so the user notices and renames.
const RENAMED_MARKET_KEYS: &[(&str, &str)] = &[
    ("mcycle_price", "min_mcycle_price"),
    ("max_stake", "max_collateral"),
    ("stake_balance_warn_threshold", "collateral_balance_warn_threshold"),
    ("stake_balance_error_threshold", "collateral_balance_error_threshold"),
    ("max_concurrent_locks", "max_concurrent_proofs"),
    ("expired_order_fulfillment_priority", "order_commitment_priority"),
];

// Keys under [market] that have been removed entirely with no replacement key.
// They are silently ignored by the broker (no `deny_unknown_fields`), so the
// wizard surfaces them so the user can clean up their broker.toml. Pairs are
// (old_key, explanation).
const REMOVED_MARKET_KEYS: &[(&str, &str)] = &[(
    "min_mcycle_price_collateral_token",
    "is no longer used; secondary fulfillment now derives its threshold from min_mcycle_price (converted to ZKC at runtime)",
)];

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

#[derive(Debug, Clone)]
struct ChainConfig {
    chain_id: u64,
    chain_name: String,
    #[allow(dead_code)]
    rpc_url: Url,
    max_collateral: String,
    min_mcycle_price: String,
    expected_probability_win_secondary_fulfillment: u32,
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
    chains: Vec<ChainConfig>,
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

        // Generate per-chain override files (only for multi-chain setups)
        if config.chains.len() > 1 {
            for chain in &config.chains {
                self.generate_chain_override_toml(chain)?;
                let broker_dir = self.broker_toml_file.parent().unwrap_or(Path::new("."));
                display.item_colored(
                    "Created",
                    broker_dir
                        .join(CHAIN_OVERRIDES_DIR)
                        .join(format!("broker.{}.toml", chain.chain_id))
                        .display(),
                    "green",
                );
            }
        }

        // Backup and generate compose.yml + prover-compose.yml
        if let Some(backup_path) = self.backup_file(&self.compose_yml_file)? {
            display.item_colored("Backup saved", backup_path.display(), "cyan");
        }
        let prover_compose_path = self.prover_compose_path();
        if let Some(backup_path) = self.backup_file(&prover_compose_path)? {
            display.item_colored("Backup saved", backup_path.display(), "cyan");
        }
        self.generate_compose_yml(&config, compose_strategy)?;
        display.item_colored("Created", self.compose_yml_file.display(), "green");
        display.item_colored("Created", prover_compose_path.display(), "green");

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
        display.step(1, 8, "Machine Configuration");

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
        display.step(2, 8, "GPU Configuration");

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
        display.step(3, 8, "Performance Benchmarking");

        let peak_prove_khz =
            self.get_peak_performance(display, global_config, segment_size).await?.floor();
        display.item_colored(
            "Setting `peak_prove_khz` to",
            format!("{:.0} kHz", peak_prove_khz),
            "green",
        );

        // Step 4: Calculated Configuration
        display.separator();
        display.step(4, 8, "Calculated Configuration");

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
        if max_concurrent_preflights < 8 {
            display.warning(&format!(
                "  Calculated max_concurrent_preflights ({max_concurrent_preflights}) is below the recommended value of 8. \
                 Consider using a machine with more CPU threads for better order throughput."
            ));
        }
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
        display.step(5, 8, "Priority Requestor Lists");

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

        // Step 6: Chain Selection
        display.separator();
        display.step(6, 8, "Chain Selection");

        display.note("Select which chains your prover should operate on.");
        display.note("All selected chains will share the same proving hardware.");
        display.note("");

        let include_testnets = Confirm::new("Include testnets?")
            .with_default(false)
            .prompt()
            .context("Failed to get testnet preference")?;

        let chain_options: Vec<&(u64, &str, bool)> = SUPPORTED_CHAINS
            .iter()
            .filter(|(_, _, is_mainnet)| *is_mainnet || include_testnets)
            .collect();

        let display_options: Vec<String> =
            chain_options.iter().map(|(id, name, _)| format!("{name} ({id})")).collect();

        // Default: all visible chains selected
        let defaults: Vec<usize> = (0..display_options.len()).collect();

        let selected_indices = MultiSelect::new("Select chains to operate on:", display_options)
            .with_default(&defaults)
            .prompt()
            .context("Failed to get chain selection")?;

        if selected_indices.is_empty() {
            bail!("At least one chain must be selected");
        }

        let _selected_chains: Vec<(u64, String)> = selected_indices
            .iter()
            .enumerate()
            .filter(|(_, selected)| !selected.is_empty())
            .map(|(i, _)| {
                let (id, name, _) = chain_options[i];
                (*id, name.to_string())
            })
            .collect();

        // MultiSelect returns the selected items, not indices — remap
        let selected_chains: Vec<(u64, String)> = selected_indices
            .iter()
            .filter_map(|selected_label| {
                chain_options.iter().find_map(|(id, name, _)| {
                    let label = format!("{name} ({id})");
                    if &label == selected_label {
                        Some((*id, name.to_string()))
                    } else {
                        None
                    }
                })
            })
            .collect();

        for (id, name) in &selected_chains {
            display.item_colored("Chain", format!("{name} ({id})"), "green");
        }

        // Step 7: Per-chain RPC URLs, Collateral & Pricing
        display.separator();
        display.step(7, 8, "Collateral & Pricing Configuration");

        let mut chain_configs: Vec<ChainConfig> = Vec::new();

        for (chain_idx, (chain_id, chain_name)) in selected_chains.iter().enumerate() {
            if selected_chains.len() > 1 {
                display.separator();
                display.note(&format!(
                    "── Configuring {} ({}) [{}/{}] ──",
                    chain_name,
                    chain_id,
                    chain_idx + 1,
                    selected_chains.len()
                ));
            }

            // Get RPC URL for this chain
            let rpc_url = self
                .get_or_prompt_chain_rpc_url(display, *chain_id, chain_name, selected_chains.len())
                .await?;

            let temp_provider = alloy::providers::ProviderBuilder::new()
                .connect(rpc_url.as_ref())
                .await
                .context(format!("Failed to connect to RPC for {chain_name}"))?;

            let detected_chain_id = temp_provider
                .get_chain_id()
                .await
                .context(format!("Failed to query chain ID for {chain_name}"))?;

            if detected_chain_id != *chain_id {
                display.warning(&format!(
                    "RPC returned chain ID {} but expected {}. Please verify the RPC URL.",
                    detected_chain_id, chain_id
                ));
                let continue_anyway = Confirm::new("Continue with this RPC anyway?")
                    .with_default(false)
                    .prompt()
                    .context("Failed to get confirmation")?;
                if !continue_anyway {
                    bail!("Chain ID mismatch for {chain_name}");
                }
            }

            // Initialize price oracle
            let price_oracle = match try_init_price_oracle(&rpc_url, *chain_id).await {
                Ok(oracle) => {
                    fetch_and_display_prices(oracle.clone(), display).await?;
                    Some(oracle)
                }
                Err(e) => {
                    display.note(&format!("⚠  Price oracle initialization failed: {}", e));
                    display
                        .note("   Collateral and pricing will be shown in single currency only.");
                    None
                }
            };

            // Collateral
            let recommended_collateral = match priority_requestor_lists.len() {
                1 => format!(
                    "{} USD",
                    boundless_market::prover_utils::config::defaults::MAX_COLLATERAL_STANDARD
                ),
                2 => "30 USD".to_string(),
                _ => "80 USD".to_string(),
            };

            display.note(&format!("Recommended max collateral: {}", recommended_collateral));
            display.note("");

            let max_collateral = prompt_validated_amount(
                &format!("Max collateral for {chain_name}:"),
                &recommended_collateral,
                "Format: '<value> ZKC' or '<value> USD'",
                &[Asset::USD, Asset::ZKC],
                "max_collateral",
            )?;

            // Pricing — check if chain has an indexer for market analysis
            display.note("");
            display.note("Analyzing market prices for competitive pricing...");

            let deployment = Deployment::from_chain_id(*chain_id)
                .context(format!("No deployment config for chain {chain_id}"))?;

            let min_mcycle_price = if deployment.has_indexer() {
                let indexer_url = deployment.clone().indexer_url.context("No indexer URL")?;
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

                match price_provider.price_percentiles().await {
                    Ok(price_percentiles) => {
                        display.item_colored(
                            "Median price",
                            format!(
                                "{} / Mcycle",
                                format_amount_with_conversion(
                                    &format!(
                                        "{} ETH",
                                        format_units(
                                            price_percentiles.p50 * U256::from(1_000_000),
                                            "ether"
                                        )?
                                    ),
                                    None,
                                    price_oracle.clone()
                                )
                                .await,
                            ),
                            "cyan",
                        );

                        let recommended = try_convert_to_usd(
                            &format!(
                                "{} ETH",
                                &format_units(
                                    price_percentiles.p25 * U256::from(1_000_000),
                                    "ether"
                                )?
                            ),
                            price_oracle.clone(),
                        )
                        .await;

                        prompt_validated_amount(
                            &format!("Minimum price per Mcycle for {chain_name}:"),
                            &recommended,
                            "Format: '<value> ETH' or '<value> USD'",
                            &[Asset::USD, Asset::ETH],
                            "min_mcycle_price",
                        )?
                    }
                    Err(e) => {
                        display.note(&format!("⚠  Failed to query market prices: {}", e));
                        Text::new(&format!("Minimum price per Mcycle for {chain_name}:"))
                            .with_default("0.00000001 ETH")
                            .with_help_message("You can update this later in broker.toml")
                            .prompt()
                            .context("Failed to get price")?
                    }
                }
            } else {
                display.note(&format!(
                    "⚠  No indexer available for {chain_name} — skipping market analysis."
                ));
                display.note("   Enter pricing manually. A reasonable starting value is");
                display.note("   0.000001 USD per Mcycle (1 cent per billion cycles).");
                display.note("");

                prompt_validated_amount(
                    &format!("Minimum price per Mcycle for {chain_name}:"),
                    "0.000001 USD",
                    "Format: '<value> ETH' or '<value> USD'. You can update this later in broker.toml",
                    &[Asset::USD, Asset::ETH],
                    "min_mcycle_price",
                )?
            };

            // Secondary fulfillment
            let expected_probability_str = Text::new(&format!(
                "Secondary fulfillment win multiplier for {chain_name} (%, default 50):"
            ))
            .with_default("50")
            .with_help_message("< 100 discounts reward, 100 = no change, > 100 boosts reward")
            .prompt()
            .context("Failed to get expected probability")?;

            let expected_probability_win_secondary_fulfillment =
                expected_probability_str.parse::<u32>().context("Invalid number format")?;

            chain_configs.push(ChainConfig {
                chain_id: *chain_id,
                chain_name: chain_name.clone(),
                rpc_url,
                max_collateral,
                min_mcycle_price,
                expected_probability_win_secondary_fulfillment,
            });

            // After first chain, offer "same for all?" shortcut
            if chain_idx == 0 && selected_chains.len() > 1 {
                let use_same = Confirm::new("Use the same collateral and pricing for all chains?")
                    .with_default(true)
                    .prompt()
                    .context("Failed to get confirmation")?;

                if use_same {
                    let first = chain_configs[0].clone();
                    for (id, name) in selected_chains.iter().skip(1) {
                        // Still need RPC URL per chain
                        let rpc = self
                            .get_or_prompt_chain_rpc_url(display, *id, name, selected_chains.len())
                            .await?;
                        chain_configs.push(ChainConfig {
                            chain_id: *id,
                            chain_name: name.clone(),
                            rpc_url: rpc,
                            max_collateral: first.max_collateral.clone(),
                            min_mcycle_price: first.min_mcycle_price.clone(),
                            expected_probability_win_secondary_fulfillment: first
                                .expected_probability_win_secondary_fulfillment,
                        });
                    }
                    break;
                }
            }
        }

        Ok(WizardConfig {
            num_threads,
            num_gpus,
            max_exec_agents,
            max_concurrent_preflights,
            max_concurrent_proofs,
            peak_prove_khz,
            segment_size,
            priority_requestor_lists,
            chains: chain_configs,
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
            search_to_block: None,
            search_from_block: None,
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

    async fn get_or_prompt_chain_rpc_url(
        &self,
        display: &DisplayManager,
        chain_id: u64,
        chain_name: &str,
        total_chains: usize,
    ) -> Result<Url> {
        // Check PROVER_RPC_URL_{chain_id} env var first
        let chain_env_var = format!("PROVER_RPC_URL_{chain_id}");
        if let Ok(rpc_url) = std::env::var(&chain_env_var) {
            let url = rpc_url.parse::<Url>().with_context(|| format!("Invalid {chain_env_var}"))?;
            display.item_colored(&format!("RPC ({chain_id})"), obscure_url(url.as_ref()), "green");
            return Ok(url);
        }

        // Single-chain: also check legacy PROVER_RPC_URL
        if total_chains == 1 {
            if let Ok(rpc_url) = std::env::var("PROVER_RPC_URL") {
                let url = rpc_url.parse::<Url>().context("Invalid PROVER_RPC_URL")?;
                display.item_colored(
                    &format!("RPC ({chain_id})"),
                    obscure_url(url.as_ref()),
                    "green",
                );
                return Ok(url);
            }
        }

        // Prompt
        let rpc_url = Text::new(&format!("RPC URL for {chain_name} ({chain_id}):"))
            .prompt()
            .context("Failed to get RPC URL")?;

        let url = rpc_url.parse::<Url>().context("Invalid RPC URL format")?;
        display.item_colored(&format!("RPC ({chain_id})"), obscure_url(url.as_ref()), "green");
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

    fn extract_price_value(s: &str) -> (f64, Option<&str>) {
        let parts: Vec<&str> = s.split_whitespace().collect();
        let value = parts.first().and_then(|v| v.parse::<f64>().ok()).unwrap_or(0.0);
        let asset = parts.get(1).copied();
        (value, asset)
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

        // Surface any keys whose serde aliases were removed — the broker will
        // refuse to load these names — and any keys that have been removed
        // outright and are now silently dropped on load.
        if matches!(strategy, FileHandlingStrategy::ModifyExisting) {
            if let Some(market) = doc.get("market").and_then(|v| v.as_table()) {
                for (old_key, new_key) in RENAMED_MARKET_KEYS {
                    if market.contains_key(old_key) {
                        display.warning(&format!(
                            "[market].{old_key} is no longer used; rename to [market].{new_key}"
                        ));
                    }
                }
                for (key, msg) in REMOVED_MARKET_KEYS {
                    if market.contains_key(key) {
                        display.warning(&format!("[market].{key} {msg}; please remove this entry"));
                    }
                }
            }
        }

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

            // Update max_collateral (uses first chain's value as base default)
            if let Some(item) = market.get_mut("max_collateral") {
                if let Some(first_chain) = config.chains.first() {
                    *item = toml_edit::value(first_chain.max_collateral.clone());
                }
            }

            // Update max_concurrent_proofs with calculation comment
            if let Some(item) = market.get_mut("max_concurrent_proofs") {
                let comment = format!("\n# Set based on GPU count: {} GPU(s)\n", config.num_gpus);
                if let Some(value) = item.as_value_mut() {
                    value.decor_mut().set_prefix(comment);
                }
                *item = toml_edit::value(config.max_concurrent_proofs as i64);
            }

            // Update priority_requestor_lists (insert if missing)
            let mut arr = toml_edit::Array::new();
            for list in &config.priority_requestor_lists {
                arr.push(list.clone());
            }
            let priority_lists_value = toml_edit::value(arr);
            if let Some(item) = market.get_mut("priority_requestor_lists") {
                *item = priority_lists_value;
            } else {
                market.insert("priority_requestor_lists", priority_lists_value);
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

            // Update min_mcycle_price (uses first chain's value as base default)
            if let Some(item) = market.get_mut("min_mcycle_price") {
                let first_chain_price = config
                    .chains
                    .first()
                    .map(|c| c.min_mcycle_price.clone())
                    .unwrap_or_else(|| "0.00000001 ETH".to_string());
                let should_update = match strategy {
                    FileHandlingStrategy::ModifyExisting => {
                        // Get existing price and compare with recommended price
                        let existing_price_str = item.as_str().unwrap_or("0");
                        let (existing_value, existing_asset) =
                            Self::extract_price_value(existing_price_str);
                        let (recommended_value, recommended_asset) =
                            Self::extract_price_value(&first_chain_price);

                        // Only compare if same asset, otherwise always update to new format
                        if existing_asset == recommended_asset && existing_value > 0.0 {
                            if existing_value <= recommended_value {
                                display.note("");
                                display.note(&format!(
                                    "Your min_mcycle_price ({}) is already competitive. Not modifying.",
                                    existing_price_str
                                ));
                                display.note("");
                                false
                            } else {
                                true
                            }
                        } else {
                            // Different assets or legacy format - update to new value
                            true
                        }
                    }
                    FileHandlingStrategy::GenerateNew => true,
                };

                if should_update {
                    *item = toml_edit::value(first_chain_price);
                }
            }

            // Update expected_probability_win_secondary_fulfillment (insert if missing,
            // uses first chain's value)
            if let Some(first_chain) = config.chains.first() {
                let probability_value = toml_edit::value(
                    first_chain.expected_probability_win_secondary_fulfillment as i64,
                );
                if let Some(item) = market.get_mut("expected_probability_win_secondary_fulfillment")
                {
                    *item = probability_value;
                } else {
                    market.insert(
                        "expected_probability_win_secondary_fulfillment",
                        probability_value,
                    );
                }
            }
        }

        // Write to file
        std::fs::write(&self.broker_toml_file, doc.to_string())
            .context("Failed to write broker.toml")?;

        Ok(())
    }

    /// Generate a per-chain override TOML file containing only per-chain fields.
    fn generate_chain_override_toml(&self, chain: &ChainConfig) -> Result<()> {
        let broker_dir = self.broker_toml_file.parent().unwrap_or(Path::new("."));
        let overrides_dir = broker_dir.join(CHAIN_OVERRIDES_DIR);
        std::fs::create_dir_all(&overrides_dir)
            .context("Failed to create chain-overrides directory")?;
        let override_path = overrides_dir.join(format!("broker.{}.toml", chain.chain_id));

        let content = format!(
            "# Per-chain override for {} ({})\n\
             # Generated by boundless-cli v{} on {}\n\
             \n\
             [market]\n\
             max_collateral = \"{}\"\n\
             min_mcycle_price = \"{}\"\n\
             expected_probability_win_secondary_fulfillment = {}\n",
            chain.chain_name,
            chain.chain_id,
            env!("CARGO_PKG_VERSION"),
            Utc::now().format("%Y-%m-%d"),
            chain.max_collateral,
            chain.min_mcycle_price,
            chain.expected_probability_win_secondary_fulfillment,
        );

        if let Some(backup_path) = self.backup_file(&override_path)? {
            tracing::info!("Backed up {} to {}", override_path.display(), backup_path.display());
        }

        std::fs::write(&override_path, content)
            .with_context(|| format!("Failed to write {}", override_path.display()))?;

        Ok(())
    }

    /// Apply wizard-derived settings (exec_agent replicas, segment size) to a compose template.
    fn apply_compose_settings(&self, content: String, config: &WizardConfig) -> Result<String> {
        let content = self.update_exec_agent_replicas(content, config.max_exec_agents)?;
        let content = self.update_segment_size(content, config.segment_size)?;
        Ok(content)
    }

    fn generate_compose_yml(
        &self,
        config: &WizardConfig,
        strategy: FileHandlingStrategy,
    ) -> Result<()> {
        // We use string manipulation instead of YAML parsing libraries because
        // compose.yml uses YAML anchors (&) and aliases (*) which are
        // not preserved by most Rust YAML libraries (serde_yaml, etc.).
        // This ensures all comments, formatting, and anchor definitions remain intact.

        // Generate compose.yml (default stack)
        let content = match strategy {
            FileHandlingStrategy::ModifyExisting => std::fs::read_to_string(&self.compose_yml_file)
                .context("Failed to read existing compose.yml")?,
            FileHandlingStrategy::GenerateNew => {
                include_str!("../../../../../compose.yml").to_string()
            }
        };
        let content = self.apply_compose_settings(content, config)?;
        std::fs::write(&self.compose_yml_file, content).context("Failed to write compose.yml")?;

        // Generate prover-compose.yml (prover stack) alongside compose.yml
        let prover_compose_path = self.prover_compose_path();
        let prover_content = match prover_compose_path.exists() {
            true => std::fs::read_to_string(&prover_compose_path)
                .context("Failed to read existing prover-compose.yml")?,
            false => include_str!("../../../../../prover-compose.yml").to_string(),
        };
        let prover_content = self.apply_compose_settings(prover_content, config)?;
        std::fs::write(&prover_compose_path, prover_content)
            .context("Failed to write prover-compose.yml")?;

        Ok(())
    }

    /// Path to prover-compose.yml, derived from the compose_yml_file directory.
    fn prover_compose_path(&self) -> PathBuf {
        let dir = self.compose_yml_file.parent().unwrap_or(Path::new("."));
        dir.join("prover-compose.yml")
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
        if config.chains.len() > 1 {
            let broker_dir = self.broker_toml_file.parent().unwrap_or(Path::new("."));
            for chain in &config.chains {
                display.item_colored(
                    &format!("  {} override", chain.chain_name),
                    broker_dir.join(format!("broker.{}.toml", chain.chain_id)).display(),
                    "green",
                );
            }
        }
        display.item_colored("  Compose config", self.compose_yml_file.display(), "green");
        display.item_colored(
            "  Prover compose config",
            self.prover_compose_path().display(),
            "green",
        );

        display.separator();
        display.note("Next steps:");
        display.note("1. Set the following environment variables:");

        // Check if env vars are set
        let private_key_set =
            std::env::var("PROVER_PRIVATE_KEY").or_else(|_| std::env::var("PRIVATE_KEY")).is_ok();
        let reward_address_set = std::env::var("REWARD_ADDRESS").is_ok();

        if private_key_set {
            display.item_colored("   PROVER_PRIVATE_KEY", "✓ Already set", "green");
        } else {
            display.note("   export PROVER_PRIVATE_KEY=<your_private_key>");
        }

        // Show per-chain RPC URL env vars for multichain, or single PROVER_RPC_URL
        if config.chains.len() > 1 {
            for chain in &config.chains {
                let env_var = format!("PROVER_RPC_URL_{}", chain.chain_id);
                if std::env::var(&env_var).is_ok() {
                    display.item_colored(&format!("   {env_var}"), "✓ Already set", "green");
                } else {
                    display.note(&format!(
                        "   export {env_var}=<{}_rpc_url>",
                        chain.chain_name.to_lowercase().replace(' ', "_")
                    ));
                }
            }
        } else {
            let prover_rpc_url_set = std::env::var("PROVER_RPC_URL").is_ok();
            let legacy_rpc_url_set = std::env::var("RPC_URL").is_ok();
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
        if config.chains.len() > 1 {
            display.note("2. Ensure you have collateral deposited on each chain:");
            for chain in &config.chains {
                display.note(&format!(
                    "   • {} ({}): min {} collateral",
                    chain.chain_name, chain.chain_id, chain.max_collateral
                ));
            }
        } else {
            display.note(&format!(
                "2. Ensure you have a minimum of {} ZKC collateral in your prover address:",
                config.chains.first().map(|c| c.max_collateral.as_str()).unwrap_or("10 USD")
            ));
        }
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
