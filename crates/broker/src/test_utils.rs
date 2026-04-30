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

//! Test-only helpers for building a [`Broker`] against a [`TestCtx`].
//!
//! Gated behind the `test-utils` feature so it stays out of release builds and
//! out of the broker crate's downstream consumers' dependency graphs.

use std::sync::Arc;

use alloy::{
    network::{AnyNetwork, Ethereum},
    providers::{fillers::ChainIdFiller, DynProvider, Provider, ProviderBuilder, WalletProvider},
};
use anyhow::Result;
use boundless_market::price_oracle::config::PriceValue;
use boundless_market::price_oracle::Amount;
use boundless_test_utils::{
    guests::{ASSESSOR_GUEST_PATH, SET_BUILDER_PATH},
    market::TestCtx,
};
use tempfile::NamedTempFile;
use url::Url;

use crate::{
    broker_sqlite_url_for_chain,
    config::{Config, ConfigWatcher},
    resolve_deployment, Broker, ChainPipeline, CoreArgs, DbObj, SqliteDb,
};

pub struct BrokerBuilder {
    args: CoreArgs,
    config_file: NamedTempFile,
    db_dir: tempfile::TempDir,
    rpc_url: Url,
}

impl BrokerBuilder {
    pub async fn new_test<P>(ctx: &TestCtx<P>, rpc_url: Url) -> Self {
        let config_file: NamedTempFile = NamedTempFile::new().unwrap();
        let mut config = Config::default();
        config.prover.set_builder_guest_path = Some(SET_BUILDER_PATH.into());
        config.prover.assessor_set_guest_path = Some(ASSESSOR_GUEST_PATH.into());
        config.market.min_mcycle_price = Amount::parse("0.0 ETH", None).unwrap();
        config.batcher.min_batch_size = 1;
        config.market.min_deadline = 30;
        config.price_oracle.eth_usd = PriceValue::Static(2500.0);
        config.price_oracle.zkc_usd = PriceValue::Static(1.0);
        config.write(config_file.path()).await.unwrap();

        let db_dir = tempfile::tempdir().unwrap();
        let db_url = format!("sqlite://{}", db_dir.path().join("broker.sqlite").display());

        let args = CoreArgs {
            db_url,
            config_file: config_file.path().to_path_buf(),
            deployment: Some(ctx.deployment.clone()),
            rpc_url: Some(rpc_url.to_string()),
            rpc_urls: Vec::new(),
            private_key: Some(ctx.prover_signer.clone()),
            bento_api_url: None,
            bonsai_api_key: None,
            bonsai_api_url: None,
            deposit_amount: None,
            rpc_retry_max: 0,
            rpc_retry_backoff: 200,
            rpc_retry_cu: 1000,
            rpc_request_timeout: 30,
            log_json: false,
            listen_only: false,
            experimental_rpc: true,
            legacy_rpc: false,
            version_registry_address: Some(ctx.version_registry_address),
            force_version_check: false,
        };
        Self { args, config_file, db_dir, rpc_url }
    }

    pub fn with_db_url(mut self, db_url: String) -> Self {
        self.args.db_url = db_url;
        self
    }

    pub async fn build<P>(
        self,
        ctx: &TestCtx<P>,
    ) -> Result<(
        Broker,
        ChainPipeline<impl Provider<Ethereum> + WalletProvider + Clone + 'static>,
        NamedTempFile,
        tempfile::TempDir,
    )>
    where
        P: Provider<Ethereum> + WalletProvider + Clone + 'static,
    {
        let config_watcher = ConfigWatcher::new(self.config_file.path()).await?;
        let config = config_watcher.config.clone();
        let (provider, any_provider, gas_priority_mode) =
            crate::build_chain_provider(&[self.rpc_url], &ctx.prover_signer, &self.args, &config)?;
        let provider = Arc::new(provider);
        let chain_id = provider.get_chain_id().await?;
        let deployment = resolve_deployment(self.args.deployment.as_ref(), chain_id)?;

        let db_url = broker_sqlite_url_for_chain(&self.args.db_url, chain_id)
            .map_err(|e| anyhow::anyhow!("invalid broker database URL: {e}"))?;
        let db: DbObj = Arc::new(
            SqliteDb::new(&db_url)
                .await
                .map_err(|e| anyhow::anyhow!("Failed to open per-chain sqlite DB: {e}"))?,
        );

        let chain = ChainPipeline {
            provider,
            any_provider,
            config,
            gas_priority_mode,
            private_key: ctx.prover_signer.clone(),
            chain_id,
            deployment,
            db,
        };

        let broker = Broker::new(self.args, config_watcher).await?;
        Ok((broker, chain, self.config_file, self.db_dir))
    }
}

pub fn make_any_provider(rpc_url: Url) -> DynProvider<AnyNetwork> {
    DynProvider::new(
        ProviderBuilder::new()
            .network::<AnyNetwork>()
            .filler(ChainIdFiller::default())
            .connect_http(rpc_url),
    )
}
