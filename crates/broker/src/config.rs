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

use std::{
    path::Path,
    sync::{Arc, RwLock},
};

use crate::{errors::CodedError, impl_coded_debug};
use anyhow::{Context, Result};
use notify::{EventKind, Watcher};
use thiserror::Error;
use tokio::{
    task::JoinHandle,
    time::{timeout, Duration},
};

// Re-export all configuration types from boundless-market
pub use boundless_market::prover_utils::{
    config_defaults as defaults, BatcherConfig, Config, MarketConfig, OrderCommitmentPriority,
    OrderPricingPriority, ProverConfig,
};

#[derive(Error)]
pub enum ConfigErr {
    #[error("Failed to lock internal config structure")]
    LockFailed,

    #[error("Invalid configuration")]
    InvalidConfig,
}

impl_coded_debug!(ConfigErr);

impl CodedError for ConfigErr {
    fn code(&self) -> &str {
        match self {
            ConfigErr::LockFailed => "[B-CON-3012]",
            ConfigErr::InvalidConfig => "[B-CON-3013]",
        }
    }
}

#[derive(Clone, Default, Debug)]
pub struct ConfigLock {
    config: Arc<RwLock<Config>>,
}

impl ConfigLock {
    fn new(config: Arc<RwLock<Config>>) -> Self {
        Self { config }
    }

    pub fn lock_all(&self) -> Result<std::sync::RwLockReadGuard<'_, Config>, ConfigErr> {
        self.config.read().map_err(|_| ConfigErr::LockFailed)
    }

    #[cfg(test)]
    pub fn load_write(&self) -> Result<std::sync::RwLockWriteGuard<'_, Config>, ConfigErr> {
        self.config.write().map_err(|_| ConfigErr::LockFailed)
    }
}

/// Max number of pending filesystem events from the config file
const FILE_MONITOR_EVENT_BUFFER: usize = 32;

/// Monitor service for watching config files for changes
pub struct ConfigWatcher {
    /// Current config data
    pub config: ConfigLock,
    /// monitor task handle
    // TODO: Need to join and monitor this handle
    _monitor: JoinHandle<Result<()>>,
}

impl ConfigWatcher {
    /// Initialize a new config watcher and handle
    pub async fn new(config_path: &Path) -> Result<Self> {
        let initial_config = Config::load(config_path).await?;
        log_deprecated_config_usage(&initial_config);
        let config = Arc::new(RwLock::new(initial_config));
        let config_copy = config.clone();
        let config_path_copy = config_path.to_path_buf();

        let startup_notification = Arc::new(tokio::sync::Notify::new());
        let startup_notification_copy = startup_notification.clone();

        let monitor = tokio::spawn(async move {
            let (tx, mut rx) = tokio::sync::mpsc::channel(FILE_MONITOR_EVENT_BUFFER);

            let mut watcher = notify::recommended_watcher(move |res| match res {
                Ok(event) => {
                    // tracing::debug!("watch event: {event:?}");
                    if let Err(err) = tx.try_send(event) {
                        // TODO we hit TrySendError::Closed if the ConfigWatcher is dropped
                        // it would be nice to auto un-watch the file and shutdown in a cleaner
                        // order
                        tracing::debug!("Failed to send filesystem event to channel: {err:?}");
                    }
                }
                Err(err) => tracing::error!("Failed to watch config file: {err:?}"),
            })
            .context("Failed to construct watcher")?;

            watcher
                .watch(&config_path_copy, notify::RecursiveMode::NonRecursive)
                .context("Failed to start watcher")?;
            startup_notification_copy.notify_one();

            while let Some(event) = rx.recv().await {
                // tracing::debug!("Got event: {event:?}");
                match event.kind {
                    EventKind::Modify(_) => {
                        tracing::debug!("Reloading modified config file");
                        let new_config = match Config::load(&config_path_copy).await {
                            Ok(val) => val,
                            Err(err) => {
                                tracing::error!("Failed to load modified config: {err:?}");
                                continue;
                            }
                        };
                        let mut config = match config_copy.write() {
                            Ok(val) => val,
                            Err(err) => {
                                tracing::error!(
                                    "Failed to lock config, previously poisoned? {err:?}"
                                );
                                continue;
                            }
                        };
                        log_deprecated_config_usage(&new_config);
                        *config = new_config;
                    }
                    _ => {
                        tracing::debug!("unsupported config file event: {event:?}");
                    }
                }
            }

            watcher.unwatch(&config_path_copy).context("Failed to stop watching config")?;

            Ok(())
        });

        // Wait for successful start up, if failed return the Result
        if let Err(err) = timeout(Duration::from_secs(1), startup_notification.notified()).await {
            tracing::error!("Failed to get notification from config monitor startup in: {err}");
            let task_res = monitor.await.context("Config watcher startup failed")?;
            match task_res {
                Ok(_) => unreachable!("Startup failed to notify in timeout but exited cleanly"),
                Err(err) => return Err(err),
            }
        }
        tracing::debug!("Successful startup");

        Ok(Self { config: ConfigLock::new(config), _monitor: monitor })
    }
}

fn log_deprecated_config_usage(config: &Config) {
    if config.market.lockin_priority_gas.is_some() {
        tracing::warn!(
            "market.lockin_priority_gas is deprecated and ignored; configure market.gas_priority_mode instead"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::primitives::Address;
    use boundless_market::dynamic_gas_filler::PriorityMode;
    use boundless_market::price_oracle::Amount;
    use std::{
        fs::File,
        io::{Seek, Write},
    };
    use tempfile::NamedTempFile;
    use tracing_test::traced_test;

    const CONFIG_TEMPL: &str = r#"
[market]
mcycle_price = "0.1 ETH"
expected_probability_win_secondary_fulfillment = 25
peak_prove_khz = 500
min_deadline = 300
lookback_blocks = 100
max_stake = "0.1 ZKC"
max_file_size = 50_000_000
min_mcycle_limit = 5

[prover]
bonsai_r0_zkvm_ver = "1.0.1"
status_poll_retry_count = 3
status_poll_ms = 1000
req_retry_count = 3
req_retry_sleep_ms = 500
proof_retry_count = 1
proof_retry_sleep_ms = 500

[batcher]
batch_max_time = 300
min_batch_size = 2
batch_max_fees = "0.1"
block_deadline_buffer_secs = 120"#;

    const CONFIG_TEMPL_2: &str = r#"
[market]
mcycle_price = "0.1 ETH"
expected_probability_win_secondary_fulfillment = 50
assumption_price = "0.1"
peak_prove_khz = 10000
min_deadline = 300
lookback_blocks = 100
max_stake = "0.1 ZKC"
max_file_size = 50_000_000
max_fetch_retries = 10
allow_client_addresses = ["0x0000000000000000000000000000000000000000"]
deny_requestor_addresses = ["0x0000000000000000000000000000000000000000"]
gas_priority_mode = "high"
max_mcycle_limit = 10
min_mcycle_limit = 5

[prover]
status_poll_retry_count = 2
status_poll_ms = 1000
req_retry_count = 1
req_retry_sleep_ms = 200
proof_retry_count = 1
proof_retry_sleep_ms = 500


[batcher]
batch_max_time = 300
batch_size = 3
block_deadline_buffer_secs = 120
txn_timeout = 45
batch_poll_time_ms = 1200
single_txn_fulfill = true
withdraw = true"#;

    const CONFIG_CUSTOM_PRIORITY_MODE: &str = r#"
[market]
mcycle_price = "0.2 ETH"
max_stake = "0.1 ZKC"
gas_priority_mode = { custom = { priority_fee_multiplier_percentage = 150, priority_fee_percentile = 15.0, dynamic_multiplier_percentage = 9 } }
"#;

    const BAD_CONFIG: &str = r#"
[market]
error = ?"#;

    fn write_config(data: &str, file: &mut File) {
        file.seek(std::io::SeekFrom::Start(0)).unwrap();
        file.write_all(data.as_bytes()).unwrap();
        file.set_len(data.len() as u64).unwrap();
    }

    #[allow(deprecated)]
    #[tokio::test]
    async fn config_parser() {
        let mut config_temp = NamedTempFile::new().unwrap();
        write_config(CONFIG_TEMPL, config_temp.as_file_mut());
        let config = Config::load(config_temp.path()).await.unwrap();

        assert_eq!(config.market.min_mcycle_price, Amount::parse("0.1 ETH", None).unwrap());
        assert_eq!(config.market.expected_probability_win_secondary_fulfillment, 25);
        assert_eq!(config.market.assumption_price, None);
        assert_eq!(config.market.peak_prove_khz, Some(500));
        assert_eq!(config.market.min_deadline, 300);
        assert_eq!(config.market.lookback_blocks, 100);
        assert_eq!(config.market.max_collateral, Amount::parse("0.1 ZKC", None).unwrap());
        assert_eq!(config.market.max_file_size, 50_000_000);
        assert_eq!(config.market.min_mcycle_limit, 5);
        assert_eq!(config.market.gas_priority_mode, PriorityMode::Medium);

        assert_eq!(config.prover.status_poll_ms, 1000);
        assert_eq!(config.prover.status_poll_retry_count, 3);
        assert_eq!(config.prover.bonsai_r0_zkvm_ver.unwrap(), "1.0.1");
        assert_eq!(config.prover.req_retry_count, 3);
        assert_eq!(config.prover.req_retry_sleep_ms, 500);
        assert_eq!(config.prover.proof_retry_count, 1);
        assert_eq!(config.prover.proof_retry_sleep_ms, 500);
        assert_eq!(config.prover.set_builder_guest_path, None);
        assert_eq!(config.prover.assessor_set_guest_path, None);

        assert_eq!(config.batcher.batch_max_time, 300);
        assert_eq!(config.batcher.min_batch_size, 2);
        assert_eq!(config.batcher.batch_max_fees, Some("0.1".into()));
        assert_eq!(config.batcher.block_deadline_buffer_secs, 120);
        assert_eq!(config.batcher.txn_timeout, 45);
        assert_eq!(config.batcher.batch_poll_time_ms, None);
    }

    #[tokio::test]
    async fn config_parser_custom_priority_mode() {
        let mut config_temp = NamedTempFile::new().unwrap();
        write_config(CONFIG_CUSTOM_PRIORITY_MODE, config_temp.as_file_mut());
        let config = Config::load(config_temp.path()).await.unwrap();

        assert_eq!(
            config.market.gas_priority_mode,
            PriorityMode::Custom {
                base_fee_multiplier_percentage: 200,
                priority_fee_multiplier_percentage: 150,
                priority_fee_percentile: 15.0,
                dynamic_multiplier_percentage: 9,
            }
        );
    }

    #[tokio::test]
    #[should_panic(expected = "TOML parse error")]
    async fn bad_config() {
        let mut config_temp = NamedTempFile::new().unwrap();
        write_config(BAD_CONFIG, config_temp.as_file_mut());
        Config::load(config_temp.path()).await.unwrap();
    }

    #[allow(deprecated)]
    #[tokio::test]
    #[traced_test]
    async fn config_watcher() {
        let mut config_temp = NamedTempFile::new().unwrap();
        write_config(CONFIG_TEMPL, config_temp.as_file_mut());
        let config_mgnr = ConfigWatcher::new(config_temp.path()).await.unwrap();

        {
            let config = config_mgnr.config.lock_all().unwrap();
            assert_eq!(config.market.min_mcycle_price, Amount::parse("0.1 ETH", None).unwrap());
            assert_eq!(config.market.expected_probability_win_secondary_fulfillment, 25);
            assert_eq!(config.market.assumption_price, None);
            assert_eq!(config.market.peak_prove_khz, Some(500));
            assert_eq!(config.market.min_deadline, 300);
            assert_eq!(config.market.lookback_blocks, 100);
            assert_eq!(config.market.max_mcycle_limit, 8000);
            assert_eq!(config.market.min_mcycle_limit, 5);
            assert_eq!(config.prover.status_poll_ms, 1000);
        }

        write_config(CONFIG_TEMPL_2, config_temp.as_file_mut());
        tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;

        {
            tracing::debug!("Locking config for reading...");
            let config = config_mgnr.config.lock_all().unwrap();
            assert_eq!(config.market.min_mcycle_price, Amount::parse("0.1 ETH", None).unwrap());
            assert_eq!(config.market.expected_probability_win_secondary_fulfillment, 50);
            assert_eq!(config.market.assumption_price, Some("0.1".into()));
            assert_eq!(config.market.peak_prove_khz, Some(10000));
            assert_eq!(config.market.min_deadline, 300);
            assert_eq!(config.market.lookback_blocks, 100);
            assert_eq!(config.market.allow_client_addresses, Some(vec![Address::ZERO]));
            assert_eq!(
                config.market.deny_requestor_addresses,
                Some([Address::ZERO].into_iter().collect())
            );
            assert_eq!(config.market.gas_priority_mode, PriorityMode::High);
            assert_eq!(config.market.max_fetch_retries, Some(10));
            assert_eq!(config.market.max_mcycle_limit, 10);
            assert_eq!(config.market.min_mcycle_limit, 5);
            assert_eq!(config.prover.status_poll_ms, 1000);
            assert_eq!(config.prover.status_poll_retry_count, 2);
            assert_eq!(config.prover.req_retry_count, 1);
            assert_eq!(config.prover.req_retry_sleep_ms, 200);
            assert_eq!(config.prover.proof_retry_count, 1);
            assert_eq!(config.prover.proof_retry_sleep_ms, 500);
            assert!(config.prover.bonsai_r0_zkvm_ver.is_none());
            assert_eq!(config.batcher.txn_timeout, 45);
            assert_eq!(config.batcher.batch_poll_time_ms, Some(1200));
            assert_eq!(config.batcher.min_batch_size, 3);
            assert!(config.batcher.single_txn_fulfill);
            assert!(config.batcher.withdraw);
        }
        tracing::debug!("closing...");
    }

    #[tokio::test]
    #[traced_test]
    #[should_panic(expected = "Failed to parse toml file")]
    async fn watcher_fail_startup() {
        let mut config_temp = NamedTempFile::new().unwrap();
        write_config(BAD_CONFIG, config_temp.as_file_mut());
        ConfigWatcher::new(config_temp.path()).await.unwrap();
    }
}
