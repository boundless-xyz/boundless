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

use alloy::network::Ethereum;
use alloy::primitives::{Address, U256};
use alloy::providers::{PendingTransactionBuilder, Provider, ProviderLayer, RootProvider};
use alloy::transports::TransportResult;

/// Configuration for the BalanceAlertLayer
#[derive(Debug, Clone, Default)]
pub struct BalanceAlertConfig {
    /// Address to periodically check the balance of
    pub watch_address: Address,
    /// Threshold at which to log a warning
    pub warn_threshold: Option<U256>,
    /// Threshold at which to log an error
    pub error_threshold: Option<U256>,
}

/// A layer that can be added to an alloy Provider
/// to log warnings and errors when the balance of a given address
/// falls below certain thresholds.
#[derive(Debug, Clone, Default)]
pub struct BalanceAlertLayer {
    config: BalanceAlertConfig,
}

/// A ProviderLayer that can be added to an alloy Provider
/// to log warnings and errors when the balance of a given address
/// falls below certain thresholds.
///
/// This checks the balance after every transaction sent via send_transaction
/// and errors, warns or trace logs accordingly
///
/// # Examples
/// ```ignore
/// let provider = ProviderBuilder::new()
///     .layer(BalanceAlertLayer::new(BalanceAlertConfig {
///         watch_address: wallet.default_signer().address(),
///         warn_threshold: parse_ether("0.1")?,
///         error_threshold: parse_ether("0.1")?,
///     }));
/// ```
impl BalanceAlertLayer {
    /// Creates a new BalanceAlertLayer with the given configuration.
    pub fn new(config: BalanceAlertConfig) -> Self {
        Self { config }
    }
}

impl<P> ProviderLayer<P> for BalanceAlertLayer
where
    P: Provider,
{
    type Provider = BalanceAlertProvider<P>;

    fn layer(&self, inner: P) -> Self::Provider {
        BalanceAlertProvider::new(inner, self.config.clone())
    }
}

/// A provider that checks the balance of a given address
/// and logs warnings and errors when the balance falls below certain thresholds.
#[derive(Clone, Debug)]
pub struct BalanceAlertProvider<P> {
    inner: P,
    config: BalanceAlertConfig,
}

impl<P> BalanceAlertProvider<P>
where
    P: Provider,
{
    #[allow(clippy::missing_const_for_fn)]
    fn new(inner: P, config: BalanceAlertConfig) -> Self {
        Self { inner, config }
    }
}

#[async_trait::async_trait]
impl<P> Provider for BalanceAlertProvider<P>
where
    P: Provider,
{
    #[inline(always)]
    fn root(&self) -> &RootProvider {
        self.inner.root()
    }

    /// Broadcasts a raw transaction RLP bytes to the network.
    ///
    /// This override checks the watched address after sending the transaction and
    /// logs a warning or error if the balance falls below the configured thresholds.
    ///
    /// See [`send_transaction`](Self::send_transaction) for more details.
    async fn send_raw_transaction(
        &self,
        encoded_tx: &[u8],
    ) -> TransportResult<PendingTransactionBuilder<Ethereum>> {
        let res = self.inner.send_raw_transaction(encoded_tx).await;
        let balance = self.inner.get_balance(self.config.watch_address).await?;

        if balance < self.config.error_threshold.unwrap_or(U256::ZERO) {
            tracing::error!(
                "balance of {} < error threshold: {}",
                self.config.watch_address,
                balance
            );
        } else if balance < self.config.warn_threshold.unwrap_or(U256::ZERO) {
            tracing::warn!(
                "balance of {} < warning threshold: {}",
                self.config.watch_address,
                balance
            );
        } else {
            tracing::trace!("balance of {} is: {}", self.config.watch_address, balance);
        }
        res
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::{
        network::{EthereumWallet, TransactionBuilder},
        node_bindings::Anvil,
        primitives::utils::parse_ether,
        providers::ProviderBuilder,
        rpc::{client::RpcClient, types::TransactionRequest},
        signers::local::LocalSigner,
    };

    async fn burn_eth(provider: impl Provider, amount: U256) -> anyhow::Result<()> {
        let tx = TransactionRequest::default().with_to(Address::ZERO).with_value(amount);
        provider.send_transaction(tx).await?.watch().await?;
        Ok(())
    }

    #[tokio::test]
    #[tracing_test::traced_test]
    async fn test_balance_alert_layer() -> anyhow::Result<()> {
        // Initial wallet balance is 10 eth, set up to warn if < 9 and error if < 5
        let anvil = Anvil::default().args(["--balance", "10"]).spawn();
        let wallet = EthereumWallet::from(LocalSigner::from(anvil.keys()[0].clone()));
        let client = RpcClient::builder().http(anvil.endpoint_url());

        let balance_alerts_layer = BalanceAlertLayer::new(BalanceAlertConfig {
            watch_address: wallet.default_signer().address(),
            warn_threshold: Some(parse_ether("9").unwrap()),
            error_threshold: Some(parse_ether("5").unwrap()),
        });

        let provider =
            ProviderBuilder::new().layer(balance_alerts_layer).wallet(wallet).on_client(client);

        burn_eth(&provider, parse_ether("0.5").unwrap()).await?;
        assert!(!logs_contain("< warning threshold")); // no log yet

        burn_eth(&provider, parse_ether("0.6").unwrap()).await?;
        assert!(logs_contain("< warning threshold"));

        burn_eth(&provider, parse_ether("6").unwrap()).await?;
        assert!(logs_contain("< error threshold"));

        Ok(())
    }

    #[tokio::test]
    #[tracing_test::traced_test]
    async fn test_balance_alert_layer_no_config() -> anyhow::Result<()> {
        // Initial wallet balance is 10 eth, set up to warn if < 9 and error if < 5
        let anvil = Anvil::default().args(["--balance", "10"]).spawn();
        let wallet = EthereumWallet::from(LocalSigner::from(anvil.keys()[0].clone()));
        let client = RpcClient::builder().http(anvil.endpoint_url());

        let balance_alerts_layer = BalanceAlertLayer::new(BalanceAlertConfig {
            watch_address: wallet.default_signer().address(),
            warn_threshold: None,
            error_threshold: None,
        });

        let provider =
            ProviderBuilder::new().layer(balance_alerts_layer).wallet(wallet).on_client(client);

        // no warning or error logs should be emitted
        burn_eth(&provider, parse_ether("0.5").unwrap()).await?;
        assert!(!logs_contain("< warning threshold"));

        burn_eth(&provider, parse_ether("0.6").unwrap()).await?;
        assert!(!logs_contain("< warning threshold"));

        burn_eth(&provider, parse_ether("6").unwrap()).await?;
        assert!(!logs_contain("< error threshold"));

        Ok(())
    }
}
