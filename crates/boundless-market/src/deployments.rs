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

use std::borrow::Cow;

use alloy::primitives::{address, Address};
use clap::Args;
use derive_builder::Builder;

pub use alloy_chains::NamedChain;

pub(crate) const BASE_MAINNET_INDEXER_URL: &str = "https://d2mdvlnmyov1e1.cloudfront.net/";
pub(crate) const BASE_SEPOLIA_INDEXER_URL: &str = "https://d3kkukmpiqlzm1.cloudfront.net/";
pub(crate) const SEPOLIA_INDEXER_URL: &str = "https://d3jjbcwhlw21k7.cloudfront.net/";

/// Configuration for a deployment of the Boundless Market.
// NOTE: See https://github.com/clap-rs/clap/issues/5092#issuecomment-1703980717 about clap usage.
#[non_exhaustive]
#[derive(Clone, Debug, Builder, Args)]
#[group(
    id = "market_deployment",
    requires = "boundless_market_address",
    requires = "set_verifier_address"
)]
pub struct Deployment {
    /// EIP-155 chain ID of the network.
    #[clap(long = "market-chain-id", env = "MARKET_CHAIN_ID")]
    #[builder(setter(into, strip_option), default)]
    pub market_chain_id: Option<u64>,

    /// Address of the [BoundlessMarket] contract.
    ///
    /// [BoundlessMarket]: crate::contracts::IBoundlessMarket
    #[clap(long, env, required = false, long_help = "Address of the BoundlessMarket contract")]
    #[builder(setter(into))]
    pub boundless_market_address: Address,

    /// Address of the [RiscZeroVerifierRouter] contract.
    ///
    /// The verifier router implements [IRiscZeroVerifier]. Each network has a canonical router,
    /// that is deployed by the core team. You can additionally deploy and manage your own verifier
    /// instead. See the [Boundless docs for more details].
    ///
    /// [RiscZeroVerifierRouter]: https://github.com/risc0/risc0-ethereum/blob/main/contracts/src/RiscZeroVerifierRouter.sol
    /// [IRiscZeroVerifier]: https://github.com/risc0/risc0-ethereum/blob/main/contracts/src/IRiscZeroVerifier.sol
    /// [Boundless docs for more details]: https://docs.boundless.network/developers/smart-contracts/verifier-contracts
    #[clap(
        long,
        env = "VERIFIER_ADDRESS",
        long_help = "Address of the RiscZeroVerifierRouter contract"
    )]
    #[builder(setter(strip_option), default)]
    pub verifier_router_address: Option<Address>,

    /// Address of the [RiscZeroSetVerifier] contract.
    ///
    /// [RiscZeroSetVerifier]: https://github.com/risc0/risc0-ethereum/blob/main/contracts/src/RiscZeroSetVerifier.sol
    #[clap(long, env, required = false, long_help = "Address of the RiscZeroSetVerifier contract")]
    #[builder(setter(into))]
    pub set_verifier_address: Address,

    /// Address of the collateral token contract. The collateral token is an ERC-20.
    #[clap(long, env)]
    #[builder(setter(strip_option), default)]
    pub collateral_token_address: Option<Address>,

    /// URL for the offchain [order stream service].
    ///
    /// [order stream service]: crate::order_stream_client
    #[clap(long, env, long_help = "URL for the offchain order stream service")]
    #[builder(setter(into, strip_option), default)]
    pub order_stream_url: Option<Cow<'static, str>>,

    /// Block number when the BoundlessMarket contract was deployed.
    /// Used as a starting point for event queries.
    #[clap(skip)]
    #[builder(setter(strip_option), default)]
    pub deployment_block: Option<u64>,

    /// Indexer URL for the market.
    #[clap(long, env, long_help = "URL for the indexer")]
    #[builder(setter(into, strip_option), default)]
    pub indexer_url: Option<Cow<'static, str>>,
}

impl Deployment {
    /// Create a new [DeploymentBuilder].
    pub fn builder() -> DeploymentBuilder {
        Default::default()
    }

    /// Lookup the [Deployment] for a named chain.
    pub const fn from_chain(chain: NamedChain) -> Option<Deployment> {
        match chain {
            NamedChain::Sepolia => Some(SEPOLIA),
            NamedChain::Base => Some(BASE),
            NamedChain::BaseSepolia => Some(BASE_SEPOLIA),
            _ => None,
        }
    }

    /// Lookup the [Deployment] by chain ID.
    pub fn from_chain_id(chain_id: impl Into<u64>) -> Option<Deployment> {
        let chain = NamedChain::try_from(chain_id.into()).ok()?;
        Self::from_chain(chain)
    }

    /// Check if the collateral token supports permit.
    /// Some chain's bridged tokens do not support permit, for example Base.
    pub fn collateral_token_supports_permit(&self) -> bool {
        self.market_chain_id.map(collateral_token_supports_permit).unwrap_or(false)
    }
}

// TODO(#654): Ensure consistency with deployment.toml and with docs
/// [Deployment] for the Sepolia testnet.
pub const SEPOLIA: Deployment = Deployment {
    market_chain_id: Some(NamedChain::Sepolia as u64),
    boundless_market_address: address!("0xc211b581cb62e3a6d396a592bab34979e1bbba7d"),
    verifier_router_address: Some(address!("0xb121b667dd2cf27f95f9f5107137696f56f188f6")),
    set_verifier_address: address!("0xcb9D14347b1e816831ECeE46EC199144F360B55c"),
    collateral_token_address: Some(address!("0xb4FC69A452D09D2662BD8C3B5BB756902260aE28")),
    order_stream_url: Some(Cow::Borrowed("https://eth-sepolia.boundless.network")),
    indexer_url: Some(Cow::Borrowed(SEPOLIA_INDEXER_URL)),
    deployment_block: None,
};

/// [Deployment] for the Base mainnet.
pub const BASE: Deployment = Deployment {
    market_chain_id: Some(NamedChain::Base as u64),
    boundless_market_address: address!("0xfd152dadc5183870710fe54f939eae3ab9f0fe82"),
    verifier_router_address: Some(address!("0xa326b2eb45a5c3c206df905a58970dca57b8719e")),
    set_verifier_address: address!("0x1Ab08498CfF17b9723ED67143A050c8E8c2e3104"),
    collateral_token_address: Some(address!("0xAA61bB7777bD01B684347961918f1E07fBbCe7CF")),
    order_stream_url: Some(Cow::Borrowed("https://base-mainnet.boundless.network")),
    indexer_url: Some(Cow::Borrowed(BASE_MAINNET_INDEXER_URL)),
    deployment_block: Some(35060420),
};

/// [Deployment] for the Base Sepolia.
pub const BASE_SEPOLIA: Deployment = Deployment {
    market_chain_id: Some(NamedChain::BaseSepolia as u64),
    boundless_market_address: address!("0x56da3786061c82214d18e634d2817e86ad42d7ce"),
    verifier_router_address: Some(address!("0xa326b2eb45a5c3c206df905a58970dca57b8719e")),
    set_verifier_address: address!("0x1Ab08498CfF17b9723ED67143A050c8E8c2e3104"),
    collateral_token_address: Some(address!("0x8d4dA4b7938471A919B08F941461b2ed1679d7bb")),
    order_stream_url: Some(Cow::Borrowed("https://base-sepolia.boundless.network")),
    indexer_url: Some(Cow::Borrowed(BASE_SEPOLIA_INDEXER_URL)),
    deployment_block: Some(30570944),
};

/// Check if the collateral token supports permit.
/// Some chain's bridged tokens do not support permit, for example Base.
pub fn collateral_token_supports_permit(chain_id: u64) -> bool {
    chain_id == 1 || chain_id == 11155111 || chain_id == 31337
}
