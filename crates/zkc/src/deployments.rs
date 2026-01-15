// Copyright 2026 Boundless Foundation, Inc.
//
// Use of this source code is governed by the Business Source License
// as found in the LICENSE-BSL file.

use alloy::primitives::{address, Address};
use clap::Args;
use derive_builder::Builder;

pub use alloy_chains::NamedChain;

/// Configuration for a deployment of the ZKC.
// NOTE: See https://github.com/clap-rs/clap/issues/5092#issuecomment-1703980717 about clap usage.
#[non_exhaustive]
#[derive(Clone, Debug, Builder, Args)]
#[group(requires = "zkc_address", requires = "vezkc_address", requires = "staking_rewards_address")]
pub struct Deployment {
    /// EIP-155 chain ID of the network.
    #[clap(long, env)]
    #[builder(setter(into, strip_option), default)]
    pub chain_id: Option<u64>,

    /// Address of the [IZKC] contract.
    ///
    /// [IZKC]: crate::contracts::IZKC
    #[clap(long, env, required = false, long_help = "Address of the ZKC contract")]
    #[builder(setter(into))]
    pub zkc_address: Address,

    /// Address of the VEZKC contract.
    #[clap(long, env, required = false, long_help = "Address of the VEZKC contract")]
    #[builder(setter(into))]
    pub vezkc_address: Address,

    /// Address of the STAKING_REWARDS contract.
    ///
    /// [IStakingRewards]: crate::contracts::IStakingRewards
    #[clap(long, env, required = false, long_help = "Address of the STAKING_REWARDS contract")]
    #[builder(setter(into))]
    pub staking_rewards_address: Address,
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
            NamedChain::Mainnet => Some(MAINNET),
            _ => None,
        }
    }

    /// Lookup the [Deployment] by chain ID.
    pub fn from_chain_id(chain_id: impl Into<u64>) -> Option<Deployment> {
        let chain = NamedChain::try_from(chain_id.into()).ok()?;
        Self::from_chain(chain)
    }
}

/// [Deployment] for the Sepolia testnet.
pub const SEPOLIA: Deployment = Deployment {
    chain_id: Some(NamedChain::Sepolia as u64),
    zkc_address: address!("0xb4FC69A452D09D2662BD8C3B5BB756902260aE28"),
    vezkc_address: address!("0xc23340732038ca6C5765763180E81B395d2e9cCA"),
    staking_rewards_address: address!("0x8af45ac61f2960a65716711d0cb922b06852a057"),
};

/// [Deployment] for the Ethereum mainnet.
pub const MAINNET: Deployment = Deployment {
    chain_id: Some(NamedChain::Mainnet as u64),
    zkc_address: address!("0x000006c2A22ff4A44ff1f5d0F2ed65F781F55555"),
    vezkc_address: address!("0xe8ae8ee8ffa57f6a79b6cbe06bafc0b05f3ffbf4"),
    staking_rewards_address: address!("0x459d87d54808fac136ddcf439fcc1d8a238311c7"),
};
