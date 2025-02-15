// Copyright (c) 2025 RISC Zero, Inc.
//
// All rights reserved.

use alloy::network::{Ethereum, Network};
use alloy::primitives::{Address, U256};
use alloy::providers::{PendingTransactionBuilder, Provider, ProviderLayer, RootProvider};
use alloy::transports::{BoxTransport, TransportResult};

/// Configuration for the BalanceAlertLayer
#[derive(Debug, Clone, Default)]
pub struct BalanceAlertConfig {
    /// Address to periodically check the balance of
    pub watch_address: Address,
    /// Threshold at which to log a warning
    pub warn_threshold: U256,
    /// Threshold at which to log an error
    pub error_threshold: U256,
}

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
    pub fn new(config: BalanceAlertConfig) -> Self {
        Self { config }
    }
}

impl<P> ProviderLayer<P, BoxTransport> for BalanceAlertLayer
where
    P: Provider,
{
    type Provider = BalanceAlertProvider<P>;

    fn layer(&self, inner: P) -> Self::Provider {
        BalanceAlertProvider::new(inner, self.config.clone())
    }
}

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
    fn root(&self) -> &RootProvider<BoxTransport> {
        self.inner.root()
    }

    async fn send_raw_transaction(
        &self,
        encoded_tx: &[u8],
    ) -> TransportResult<PendingTransactionBuilder<BoxTransport, Ethereum>> {
        let res = self.inner.send_raw_transaction(encoded_tx).await;
        let balance = self.inner.get_balance(self.config.watch_address).await?;

        if balance < self.config.error_threshold {
            tracing::error!(
                "balance of {} < warning threshold: {}",
                self.config.watch_address,
                balance
            );
        } else if balance < self.config.warn_threshold {
            tracing::warn!(
                "balance of {} < error threshold: {}",
                self.config.watch_address,
                balance
            );
        } else {
            tracing::trace!("balance of {} is: {}", self.config.watch_address, balance);
        }
        res
    }
}
