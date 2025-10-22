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

use anyhow::Result;
use clap::Args;
use colored::Colorize;

use crate::commands::setup::secrets::{address_from_private_key, obscure_url};
use crate::config::GlobalConfig;
use crate::config_file::{Config, Secrets};

/// Show requestor configuration status
#[derive(Args, Clone, Debug)]
pub struct RequestorConfigCmd {}

impl RequestorConfigCmd {
    /// Run the command
    pub async fn run(&self, _global_config: &GlobalConfig) -> Result<()> {
        println!("\n{}\n", "Requestor Module".bold().underline());

        let config = Config::load().ok();
        let secrets = Secrets::load().ok();

        if let Some(ref cfg) = config {
            if let Some(ref requestor) = cfg.requestor {
                let display_network = match requestor.network.as_str() {
                    "base-mainnet" => "Base Mainnet",
                    "base-sepolia" => "Base Sepolia",
                    "eth-sepolia" => "Ethereum Sepolia",
                    custom => custom,
                };

                println!("{} {}", "Network:".bold(), display_network.blue());

                // Check env var first, then config
                let env_requestor_pk = std::env::var("REQUESTOR_PRIVATE_KEY").ok();
                let requestor_sec = secrets.as_ref().and_then(|s| s.requestor_networks.get(&requestor.network));

                if let Some(ref rpc) = requestor_sec.and_then(|s| s.rpc_url.as_ref()) {
                    println!("{} {}", "RPC URL:".bold(), obscure_url(rpc).dimmed());
                }

                let (pk, pk_source) = if let Some(ref env_pk) = env_requestor_pk {
                    (Some(env_pk.as_str()), "env")
                } else {
                    (requestor_sec.and_then(|s| s.private_key.as_deref()), "config")
                };

                if let Some(pk_str) = pk {
                    if let Some(addr) = address_from_private_key(pk_str) {
                        println!("{} {}", "Requestor Address:".bold(), format!("{:#x}", addr).green());
                    }
                    println!("{} {} {}", "Private Key:".bold(), "Configured".green(), format!("[{}]", pk_source).dimmed());
                } else if let Some(addr) = requestor_sec.and_then(|s| s.address.as_deref()) {
                    println!("{} {}", "Requestor Address:".bold(), addr.green());
                    println!(
                        "{} {} {}",
                        "Private Key:".bold(),
                        "Not configured (read-only)".yellow(),
                        "[config file]".dimmed()
                    );
                } else {
                    println!(
                        "{} {}",
                        "Private Key:".bold(),
                        "Not configured (read-only)".yellow()
                    );
                }
            } else {
                println!("{}", "Not configured - run 'boundless requestor setup'".red());
            }
        } else {
            println!("{}", "Not configured - run 'boundless requestor setup'".red());
        }

        println!(
            "\n💡 {} {}",
            "Tip:".bold(),
            "Run 'boundless requestor --help' to see available commands".dimmed()
        );
        println!();

        Ok(())
    }
}
