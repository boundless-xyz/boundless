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

use anyhow::Result;
use clap::Args;

use crate::commands::config_display::{
    display_address_and_key_status, display_not_configured, display_rpc_url, display_tip,
    get_private_key_with_source, get_rpc_url_with_source, normalize_network_name, ModuleType,
};
use crate::config::GlobalConfig;
use crate::config_file::{Config, Secrets};
use crate::display::DisplayManager;

/// Show prover configuration status
#[derive(Args, Clone, Debug)]
pub struct ProverConfigCmd {}

impl ProverConfigCmd {
    /// Run the command
    pub async fn run(&self, _global_config: &GlobalConfig) -> Result<()> {
        let module = ModuleType::Prover;
        let display = DisplayManager::new();

        display.header(module.display_name());

        let config = Config::load().ok();
        let secrets = Secrets::load().ok();

        let Some(ref cfg) = config else {
            display_not_configured(&display, module);
            display_tip(&display, module);
            return Ok(());
        };

        let Some(ref prover) = cfg.prover else {
            display_not_configured(&display, module);
            display_tip(&display, module);
            return Ok(());
        };

        let network = normalize_network_name(&prover.network);
        display.item_colored("Network", network, "cyan");

        let prover_sec = secrets.as_ref().and_then(|s| s.prover_networks.get(&prover.network));

        let (rpc_url, rpc_source) = get_rpc_url_with_source(
            module.rpc_url_env_var(),
            prover_sec.and_then(|s| s.rpc_url.as_deref()),
        );
        display_rpc_url(&display, rpc_url, rpc_source);

        let (pk, pk_source) = get_private_key_with_source(
            module.private_key_env_var(),
            prover_sec.and_then(|s| s.private_key.as_deref()),
        );

        display_address_and_key_status(
            &display,
            module.address_label(),
            pk,
            pk_source,
            prover_sec.and_then(|s| s.address.as_deref()),
        );

        display_tip(&display, module);

        Ok(())
    }
}
