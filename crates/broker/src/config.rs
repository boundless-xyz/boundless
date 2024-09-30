// Copyright (c) 2024 RISC Zero, Inc.
//
// All rights reserved.

use std::{
    path::{Path, PathBuf},
    sync::{Arc, RwLock},
};

use alloy::primitives::B256;
use anyhow::{Context, Result};
use notify::{EventKind, Watcher};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::{
    fs,
    task::JoinHandle,
    time::{timeout, Duration},
};

/// All configuration related to markets mechanics
#[derive(Deserialize, Serialize)]
pub struct MarketConf {
    /// Mega Cycle price (in native token)
    pub mcycle_price: String,
    /// Assumption price (in native token)
    pub assumption_price: String,
    /// Peak single proof performance in kHz
    ///
    /// Used for sanity checking bids to prevent slashing
    pub peak_prove_khz: Option<u64>,
    /// Parallel proofs
    ///
    /// Should be set to executor count in Bento or parallel proofs in bonsai
    pub parallel_proofs: u64,
    /// Min blocks allowed to consider bidding on the proof
    pub min_deadline: u64,
    /// Order lookback blocks
    ///
    /// On startup the number of blocks to look back for possible open orders
    pub lookback_blocks: u64,
    /// Max stake amount, in (native token)
    pub max_stake: String,
    /// ImageID's that skip preflight
    pub skip_preflight_ids: Option<Vec<B256>>,
    /// lockinRequest priority gas
    ///
    /// Optional additional gas to add to the transaction for lockinRequest, good
    /// for increasing the priority if competing with multiple provers during the
    /// same block
    pub lockin_priority_gas: Option<u128>,
    /// Max input / image file size
    pub max_file_size: usize,
}

impl Default for MarketConf {
    fn default() -> Self {
        Self {
            mcycle_price: "0.1".to_string(),
            assumption_price: "0.1".to_string(),
            peak_prove_khz: None,
            parallel_proofs: 1,
            min_deadline: 150, // ~300 seconds aka 5 mins
            lookback_blocks: 100,
            max_stake: "0.1".to_string(),
            skip_preflight_ids: None,
            lockin_priority_gas: None,
            max_file_size: 50_000_000,
        }
    }
}

/// All configuration related to prover (bonsai / Bento) mechanics
#[derive(Deserialize, Serialize)]
pub struct ProverConf {
    /// Polling interval to monitor proving status (in millisecs)
    pub status_poll_ms: u64,
    /// Optional config, if using bonsai set the zkvm version here
    pub bonsai_r0_zkvm_ver: Option<String>,
    /// Number of retries to query a prover backend for on failures
    ///
    /// Provides a little durability for transient failures during proof status requests
    pub req_retry_count: u64,
    /// Aggregator guest ELF path
    ///
    /// When using a durable deploy, set this to the published current SOT guest ELF path on the
    /// system
    pub agg_set_guest_path: Option<PathBuf>,
    /// Assessor ELF path
    pub assessor_set_guest_path: Option<PathBuf>,
}

impl Default for ProverConf {
    fn default() -> Self {
        Self {
            status_poll_ms: 1000,
            bonsai_r0_zkvm_ver: None,
            req_retry_count: 0,
            agg_set_guest_path: None,
            assessor_set_guest_path: None,
        }
    }
}

/// All configuration related to batching / aggregation
#[derive(Deserialize, Serialize)]
pub struct BatcherConfig {
    /// Max batch duration before publishing (in seconds)
    pub batch_max_time: Option<u64>,
    /// Batch size (in proofs) before publishing
    pub batch_size: Option<u64>,
    /// max batch fees (in ETH) before publishing
    pub batch_max_fees: Option<String>,
    /// Batch blocktime buffer
    ///
    /// Number of seconds before the lowest block deadline in the order batch
    /// to flush the batch. This should be approximately snark_proving_time * 2
    pub block_deadline_buffer_secs: u64,
}

impl Default for BatcherConfig {
    fn default() -> Self {
        Self {
            batch_max_time: None,
            batch_size: Some(2),
            batch_max_fees: None,
            block_deadline_buffer_secs: 120,
        }
    }
}

/// Top level config for the broker service
#[derive(Deserialize, Serialize, Default)]
pub struct Config {
    /// Market / bidding configurations
    pub market: MarketConf,
    /// Prover backend configs
    pub prover: ProverConf,
    /// Aggregation batch configs
    pub batcher: BatcherConfig,
}

impl Config {
    /// Load the config from disk
    pub async fn load(path: &Path) -> Result<Self> {
        let data = fs::read_to_string(path).await.context("Failed to read config file")?;
        toml::from_str(&data).context("Failed to parse toml file")
    }

    /// Write the config to disk
    #[cfg(feature = "test-utils")]
    pub async fn write(&self, path: &Path) -> Result<()> {
        let data = toml::to_string(&self).context("Failed to serialize config")?;
        fs::write(path, data).await.context("Failed to write Config to disk")
    }
}

#[derive(Error, Debug)]
pub enum ConfigErr {
    #[error("Failed to lock internal config structure")]
    LockFailed,

    #[error("Invalid configuration")]
    InvalidConfig,
}

#[derive(Clone, Default)]
pub struct ConfigLock {
    config: Arc<RwLock<Config>>,
}

impl ConfigLock {
    fn new(config: Arc<RwLock<Config>>) -> Self {
        Self { config }
    }

    pub fn lock_all(&self) -> Result<std::sync::RwLockReadGuard<Config>, ConfigErr> {
        self.config.read().map_err(|_| ConfigErr::LockFailed)
    }

    #[cfg(test)]
    pub fn load_write(&self) -> Result<std::sync::RwLockWriteGuard<Config>, ConfigErr> {
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
        let config = Arc::new(RwLock::new(Config::load(config_path).await?));
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

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::hex::FromHex;
    use std::{
        fs::File,
        io::{Seek, Write},
    };
    use tempfile::NamedTempFile;
    use tracing_test::traced_test;

    const CONFIG_TEMPL: &str = r#"
[market]
mcycle_price = "0.1"
assumption_price = "0.1"
peak_prove_khz = 500
parallel_proofs = 1
min_deadline = 150
lookback_blocks = 100
max_stake = "0.1"
skip_preflight_ids = ["0x0000000000000000000000000000000000000000000000000000000000000001"]
max_file_size = 50_000_000

[prover]
status_poll_ms = 1000
bonsai_r0_zkvm_ver = "1.0.1"
req_retry_count = 0

[batcher]
batch_max_time = 300
batch_size = 2
batch_max_fees = "0.1"
block_deadline_buffer_secs = 120"#;

    const CONFIG_TEMPL_2: &str = r#"
[market]
mcycle_price = "0.1"
assumption_price = "0.1"
peak_prove_khz = 10000
parallel_proofs = 1
min_deadline = 150
lookback_blocks = 100
max_stake = "0.1"
skip_preflight_ids = ["0x0000000000000000000000000000000000000000000000000000000000000001"]
max_file_size = 50_000_000

[prover]
status_poll_ms = 1000
req_retry_count = 0

[batcher]
batch_max_time = 300
batch_size = 2
block_deadline_buffer_secs = 120"#;

    const BAD_CONFIG: &str = r#"
[market]
error = ?"#;

    fn write_config(data: &str, file: &mut File) {
        file.seek(std::io::SeekFrom::Start(0)).unwrap();
        file.write_all(data.as_bytes()).unwrap();
        file.set_len(data.len() as u64).unwrap();
    }

    #[tokio::test]
    async fn config_parser() {
        let mut config_temp = NamedTempFile::new().unwrap();
        write_config(CONFIG_TEMPL, config_temp.as_file_mut());
        let config = Config::load(config_temp.path()).await.unwrap();

        assert_eq!(config.market.mcycle_price, "0.1");
        assert_eq!(config.market.assumption_price, "0.1");
        assert_eq!(config.market.peak_prove_khz, Some(500));
        assert_eq!(config.market.parallel_proofs, 1);
        assert_eq!(config.market.min_deadline, 150);
        assert_eq!(config.market.lookback_blocks, 100);
        assert_eq!(config.market.max_stake, "0.1");
        assert_eq!(config.market.max_file_size, 50_000_000);
        assert_eq!(
            config.market.skip_preflight_ids.unwrap()[0],
            B256::from_hex("0x0000000000000000000000000000000000000000000000000000000000000001")
                .unwrap()
        );
        assert_eq!(config.market.lockin_priority_gas, None);

        assert_eq!(config.prover.status_poll_ms, 1000);
        assert_eq!(config.prover.bonsai_r0_zkvm_ver.unwrap(), "1.0.1");
        assert_eq!(config.prover.req_retry_count, 0);
        assert_eq!(config.prover.agg_set_guest_path, None);
        assert_eq!(config.prover.assessor_set_guest_path, None);

        assert_eq!(config.batcher.batch_max_time, Some(300));
        assert_eq!(config.batcher.batch_size, Some(2));
        assert_eq!(config.batcher.batch_max_fees, Some("0.1".into()));
        assert_eq!(config.batcher.block_deadline_buffer_secs, 120);
    }

    #[tokio::test]
    #[should_panic(expected = "TOML parse error")]
    async fn bad_config() {
        let mut config_temp = NamedTempFile::new().unwrap();
        write_config(BAD_CONFIG, config_temp.as_file_mut());
        Config::load(config_temp.path()).await.unwrap();
    }

    #[tokio::test]
    #[traced_test]
    async fn config_watcher() {
        let mut config_temp = NamedTempFile::new().unwrap();
        write_config(CONFIG_TEMPL, config_temp.as_file_mut());
        let config_mgnr = ConfigWatcher::new(config_temp.path()).await.unwrap();

        {
            let config = config_mgnr.config.lock_all().unwrap();
            assert_eq!(config.market.mcycle_price, "0.1");
            assert_eq!(config.market.assumption_price, "0.1");
            assert_eq!(config.market.peak_prove_khz, Some(500));
            assert_eq!(config.market.parallel_proofs, 1);
            assert_eq!(config.market.min_deadline, 150);
            assert_eq!(config.market.lookback_blocks, 100);
            assert_eq!(config.prover.status_poll_ms, 1000);
        }

        write_config(CONFIG_TEMPL_2, config_temp.as_file_mut());
        tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;

        {
            tracing::debug!("Locking config for reading...");
            let config = config_mgnr.config.lock_all().unwrap();
            assert_eq!(config.market.mcycle_price, "0.1");
            assert_eq!(config.market.assumption_price, "0.1");
            assert_eq!(config.market.peak_prove_khz, Some(10000));
            assert_eq!(config.market.parallel_proofs, 1);
            assert_eq!(config.market.min_deadline, 150);
            assert_eq!(config.market.lookback_blocks, 100);
            assert_eq!(config.prover.status_poll_ms, 1000);
            assert!(config.prover.bonsai_r0_zkvm_ver.is_none());
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
