// Copyright 2026 Boundless Foundation, Inc.
//
// Use of this source code is governed by the Business Source License
// as found in the LICENSE-BSL file.

//! Deployment configuration types and values for PoVW and ZKC contracts.

use alloy_primitives::{address, Address};
use clap::Args;
use derive_builder::Builder;

pub use alloy_chains::NamedChain;

/// Configuration for a deployment of PoVW and ZKC contracts.
#[non_exhaustive]
#[derive(Clone, Debug, Builder, Args)]
#[group(id = "povw_deployment", requires = "povw_accounting_address")]
pub struct Deployment {
    /// EIP-155 chain ID of the network.
    #[clap(long = "povw-chain-id", env = "POVW_CHAIN_ID")]
    #[builder(setter(into, strip_option), default)]
    pub povw_chain_id: Option<u64>,

    /// Address of the PoVW accounting contract.
    #[clap(long, env = "POVW_ACCOUNTING_ADDRESS", required = false)]
    #[builder(setter(into), default)]
    pub povw_accounting_address: Address,

    /// Address of the PoVW mint contract.
    #[clap(long, env = "POVW_MINT_ADDRESS", required = false)]
    #[builder(setter(into), default)]
    pub povw_mint_address: Address,

    /// Address of the ZKC token contract.
    #[clap(long, env = "ZKC_ADDRESS", required = false)]
    #[builder(setter(into), default)]
    pub zkc_address: Address,

    /// Address of the veZKC contract.
    #[clap(long, env = "VEZKC_ADDRESS", required = false)]
    #[builder(setter(into), default)]
    pub vezkc_address: Address,
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
    povw_chain_id: Some(NamedChain::Sepolia as u64),
    povw_accounting_address: address!("0xC5E956732F4bA6B1973a859Cf382244db6e84D0b"),
    povw_mint_address: address!("0xc98218AafE225035a34795Bf4f6777b7d541E326"),
    zkc_address: address!("0xb4FC69A452D09D2662BD8C3B5BB756902260aE28"),
    vezkc_address: address!("0xc23340732038ca6C5765763180E81B395d2e9cCA"),
};

/// [Deployment] for the Ethereum mainnet.
pub const MAINNET: Deployment = Deployment {
    povw_chain_id: Some(NamedChain::Mainnet as u64),
    povw_accounting_address: address!("0x319bd4050b2170a7aE3Ead3E6d5AB8a5c7cFBDF8"),
    povw_mint_address: address!("0xBFCE7c2d5e7EdDEab71B3eeED770713c8b755397"),
    zkc_address: address!("0x000006c2A22ff4A44ff1f5d0F2ed65F781F55555"),
    vezkc_address: address!("0xE8Ae8eE8ffa57F6a79B6Cbe06BAFc0b05F3ffbf4"),
};
