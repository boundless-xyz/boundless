use alloy_primitives::ChainId;
use alloy_sol_types::sol;

#[cfg(feature = "build-guest")]
pub use crate::guest_artifacts::FULFILLMENT_COUNTER_GUEST_PATH;
pub use crate::guest_artifacts::{FULFILLMENT_COUNTER_GUEST_ELF, FULFILLMENT_COUNTER_GUEST_ID};

#[cfg(feature = "build-guest")]
mod guest_artifacts {
    include!(concat!(env!("OUT_DIR"), "/methods.rs"));
}

#[cfg(not(feature = "build-guest"))]
mod guest_artifacts {
    pub const FULFILLMENT_COUNTER_GUEST_ELF: &[u8] =
        include_bytes!("../elfs/fulfillment-counter-guest.bin");
    pub const FULFILLMENT_COUNTER_GUEST_ID: [u32; 8] =
        bytemuck::must_cast(*include_bytes!("../elfs/fulfillment-counter-guest.iid"));
}

// HACK: Defining a Steel::Commitment symbol here allows resolution of the Steel.Commitment
// reference in the sol! macro below.
#[expect(non_snake_case)]
mod Steel {
    pub(super) use risc0_steel::Commitment;
}

sol! {
    /// Minimal interface defining just the event we need from IBoundlessMarket.
    interface IBoundlessMarket {
        event RequestFulfilled(uint256 indexed requestId, address indexed prover, bytes32 requestDigest);
    }

    /// Journal committed by the fulfillment counter guest.
    struct FulfillmentCounterJournal {
        address prover;
        address market;
        uint64 startBlock;
        uint64 endBlock;
        uint64 count;
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
