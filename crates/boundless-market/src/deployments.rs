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

use std::borrow::Cow;

use alloy::primitives::{address, Address};
use derive_builder::Builder;

pub use alloy_chains::NamedChain;

/// Configuration for a deployment of the Boundless Market.
#[non_exhaustive]
#[derive(Clone, Debug, Builder)]
pub struct Deployment {
    /// Address of the [BoundlessMarket] contract.
    ///
    /// [BoundlessMarket]: crate::contracts::boundless_market_contract
    pub boundless_market_address: Address,

    /// Address of the [RiscZeroVerifierRouter] contract.
    ///
    /// The verifier router implements [IRiscZeroVerifier]. Each network has a canonical router,
    /// that is deployed by the core team. You can additionall deploy and manage your own verifier
    /// instead. See the [Boundless docs for more details].
    ///
    /// [RiscZeroVerifierRouter]: https://github.com/risc0/risc0-ethereum/blob/main/contracts/src/RiscZeroVerifierRouter.sol
    /// [IRiscZeroVerifier]: https://github.com/risc0/risc0-ethereum/blob/main/contracts/src/IRiscZeroVerifier.sol
    /// [Boundless docs for more details]: https://docs.beboundless.xyz/developers/smart-contracts/verifier-contracts
    pub verifier_router_address: Address,

    /// Address of the [RiscZeroSetVerifier] contract.
    ///
    /// [RiscZeroSetVerifier]: https://github.com/risc0/risc0-ethereum/blob/main/contracts/src/RiscZeroSetVerifier.sol
    pub set_verifier_address: Address,

    /// URL for the offchain [order stream service].
    ///
    /// [order stream service]: crate::order_stream_client
    #[builder(setter(into))]
    pub order_stream_url: Cow<'static, str>,
}

impl Deployment {
    /// Lookup the [Deployment] for a named chain.
    pub const fn from_chain(chain: NamedChain) -> Option<Deployment> {
        match chain {
            NamedChain::Sepolia => Some(SEPOLIA),
            _ => None,
        }
    }

    /// Lookup the [Deployment] by chain ID.
    pub fn from_chain_id(chain_id: impl Into<u64>) -> Option<Deployment> {
        let chain = NamedChain::try_from(chain_id.into()).ok()?;
        Self::from_chain(chain)
    }
}

// TODO: would it be possible to have a single source of truth here relative to the
// deployment.toml?
/// [Deployment] for the Sepolia testnet.
pub const SEPOLIA: Deployment = Deployment {
    boundless_market_address: address!("0x006b92674E2A8d397884e293969f8eCD9f615f4C"),
    verifier_router_address: address!("0x925d8331ddc0a1F0d96E68CF073DFE1d92b69187"),
    set_verifier_address: address!("0xad2c6335191EA71Ffe2045A8d54b93A851ceca77"),
    order_stream_url: Cow::Borrowed("https://eth-sepolia.beboundless.xyz"),
};
