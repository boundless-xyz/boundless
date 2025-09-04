// Copyright 2025 RISC Zero, Inc.
//
// Use of this source code is governed by the Business Source License
// as found in the LICENSE-BSL file.

//! Deployment configuration types and values for PoVW and ZKC contracts.

use alloy_primitives::Address;
use clap::Args;
use derive_builder::Builder;

pub use alloy_chains::NamedChain;

/// Configuration for a deployment of PoVW and ZKC contracts.
#[non_exhaustive]
#[derive(Clone, Debug, Builder, Args)]
#[group(requires = "povw_accounting_address")]
pub struct Deployment {
    /// EIP-155 chain ID of the network.
    #[clap(long, env)]
    #[builder(setter(into, strip_option), default)]
    pub chain_id: Option<u64>,

    /// Address of the PoVW accounting contract.
    #[clap(long, env = "POVW_ACCOUNTING_ADDRESS")]
    #[builder(setter(into))]
    pub povw_accounting_address: Address,

    /// Address of the PoVW mint contract.
    #[clap(long, env = "POVW_MINT_ADDRESS")]
    #[builder(setter(into), default)]
    pub povw_mint_address: Address,

    /// Address of the ZKC token contract.
    #[clap(long, env = "ZKC_ADDRESS")]
    #[builder(setter(into), default)]
    pub zkc_address: Address,

    /// Address of the veZKC contract.
    #[clap(long, env = "VEZKC_ADDRESS")]
    #[builder(setter(into), default)]
    pub vezkc_address: Address,
}

impl Deployment {
    /// Create a new [DeploymentBuilder].
    pub fn builder() -> DeploymentBuilder {
        Default::default()
    }

    /// Lookup the [Deployment] for a named chain.
    pub const fn from_chain(_chain: NamedChain) -> Option<Deployment> {
        None
    }

    /// Lookup the [Deployment] by chain ID.
    pub fn from_chain_id(chain_id: impl Into<u64>) -> Option<Deployment> {
        let chain = NamedChain::try_from(chain_id.into()).ok()?;
        Self::from_chain(chain)
    }
}
