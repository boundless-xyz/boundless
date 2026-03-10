use alloy_primitives::ChainId;
use alloy_sol_types::sol;

#[cfg(feature = "build-guest")]
pub use crate::guest_artifacts::MARKET_CONTRIBUTION_CALCULATOR_GUEST_PATH;
pub use crate::guest_artifacts::{
    MARKET_CONTRIBUTION_CALCULATOR_GUEST_ELF, MARKET_CONTRIBUTION_CALCULATOR_GUEST_ID,
};

#[cfg(feature = "build-guest")]
mod guest_artifacts {
    include!(concat!(env!("OUT_DIR"), "/methods.rs"));
}

#[cfg(not(feature = "build-guest"))]
mod guest_artifacts {
    pub const MARKET_CONTRIBUTION_CALCULATOR_GUEST_ELF: &[u8] =
        include_bytes!("../elfs/market-contribution-calculator-guest.bin");
    pub const MARKET_CONTRIBUTION_CALCULATOR_GUEST_ID: [u32; 8] =
        bytemuck::must_cast(*include_bytes!("../elfs/market-contribution-calculator-guest.iid"));
}

// HACK: Defining a Steel::Commitment symbol here allows resolution of the Steel.Commitment
// reference in the sol! macro below.
#[expect(non_snake_case)]
mod Steel {
    pub(super) use risc0_steel::Commitment;
}

sol! {
    /// Minimal interface for reading the cumulative contribution state from BoundlessMarket.
    interface IBoundlessMarket {
        function contributingProversLength() external view returns (uint256);
        function contributingProvers(uint256 index) external view returns (address);
        function proverCumulativeContribution(address prover) external view returns (uint256);
    }

    /// A prover's market contribution delta over an epoch range.
    struct MarketContribution {
        address prover;
        uint256 contribution;
    }

    /// Journal committed by the market contribution calculator guest.
    struct MarketContributionJournal {
        MarketContribution[] contributions;
        address marketAddress;
        uint256 epochStart;
        uint256 epochEnd;
        Steel.Commitment steelCommit;
    }
}

/// Resolve a chain ID to an OP [ChainSpec] reference.
///
/// Supports Base Mainnet (8453), Base Sepolia (84532), OP Mainnet (10), and OP Sepolia (11155420).
pub fn op_chain_spec_from_id(
    chain_id: ChainId,
) -> Option<&'static risc0_op_steel::optimism::OpChainSpec> {
    use risc0_op_steel::optimism::*;

    match chain_id {
        8453 => Some(&BASE_MAINNET_CHAIN_SPEC),
        84532 => Some(&BASE_SEPOLIA_CHAIN_SPEC),
        10 => Some(&OP_MAINNET_CHAIN_SPEC),
        11155420 => Some(&OP_SEPOLIA_CHAIN_SPEC),
        _ => None,
    }
}
