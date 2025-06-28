use std::collections::{btree_map, BTreeMap};

use risc0_zkvm::guest::env;

use alloy_primitives::{address, Address, B256, U256};
use alloy_sol_types::sol;
use risc0_steel::{
    ethereum::{EthEvmEnv, EthEvmInput, ETH_SEPOLIA_CHAIN_SPEC},
    Commitment, Contract, Event, EvmBlockHeader, EvmEnv,
};
use serde::{Deserialize, Serialize};

/// Address of the deployed contract to query.
const POVW_CONTRACT_ADDRESS: Address = address!("0x0000000000000000000000000000000000000001");

sol! {
    // Copied from contracts/src/povw/PoVW.sol
    interface IPoVW {
        event EpochFinalized(uint256 indexed epoch, uint256 totalWork);
        event WorkLogUpdated(
            address indexed logId, uint256 epochNumber, bytes32 initialCommit, bytes32 updatedCommit, uint256 work
        );
    }

    // Copied from contracts/src/povw/Mint.sol
    struct MintCalculatorUpdate {
        address workLogId;
        bytes32 initialCommit;
        bytes32 finalCommit;
    }

    struct MintCalculatorMint {
        address recipient;
        uint256 value;
    }

    struct MintCalculatorJournal {
        MintCalculatorMint[] mints;
        MintCalculatorUpdate[] updates;
        Commitment steelCommit;
    }
}

#[derive(Serialize, Deserialize)]
struct Input {
    /// Mapping of block number to [EthEvmInput] for each block accessed in this execution.
    pub env: Vec<EthEvmInput>,
}

/// The mint calculator ensures:
/// * An event was logged by the PoVW contract for each log update and epoch finalization.
///   * Each event is counted at most once.
///   * Events from an unbroken chain from initialCommit to finalCommit. This constitutes an
///     exhaustiveness check such that the prover cannot exclude updates, and thereby deny a reward.
/// * Mint value is calculated correctly from the PoVW totals in each included epoch.
///   * An event was logged by the PoVW contract for epoch finalization.
///   * The total work from the epoch finalization event is used in the mint calculation.
///   * The mint recipient is set correctly.
fn main() {
    // Read the input from the guest environment.
    // TODO: Use postcard another codec instead.
    let input: Input = env::read();

    // Converts the input into a `EvmEnv` structs for execution.
    let envs: BTreeMap<u64, EthEvmEnv<_, _>> = input
        .env
        .into_iter()
        .map(|env_input| {
            let env = env_input.into_env(&ETH_SEPOLIA_CHAIN_SPEC);
            (env.header().number(), env)
        })
        .collect();

    let mut epochs = BTreeMap::<u32, U256>::new();
    for (block_number, env) in envs.iter() {
        // Query all `EpochFinalized` events of the PoVW contract.
        // TODO: This is possibly wasteful, in that only a subset will have this event.
        let epoch_finalized_events =
            Event::new::<IPoVW::EpochFinalized>(&env).address(POVW_CONTRACT_ADDRESS).query();

        for epoch_finalized_event in epoch_finalized_events {
            epochs
                .insert(epoch_finalized_event.epoch.to::<u32>(), epoch_finalized_event.totalWork)
                .is_none_or(|_| panic!("multiple epoch finalized events"));
        }
    }

    let mut mints = BTreeMap::<Address, U256>::new();
    let mut updates = BTreeMap::<Address, (B256, B256)>::new();
    for (block_number, env) in envs.iter() {
        // Query all `WorkLogUpdated` events of the PoVW contract.
        // TODO: This is possibly wasteful, in that only a subset will have this event.
        let update_events =
            Event::new::<IPoVW::WorkLogUpdated>(&env).address(POVW_CONTRACT_ADDRESS).query();

        for update_event in update_events {
            match updates.entry(update_event.logId) {
                btree_map::Entry::Vacant(entry) => {
                    entry.insert((update_event.initialCommit, update_event.updatedCommit));
                }
                btree_map::Entry::Occupied(entry) => {
                    assert_eq!(
                        entry.get().1,
                        update_event.initialCommit,
                        "multiple update events for {} do not form a chain",
                        update_event.logId
                    );
                    entry.get_mut().1 = update_event.updatedCommit;
                }
            }

            // TODO: Consider minting to an address that is not nessesarily the log id.
            // TODO: Weight this by the finalized epoch total.
            *mints.entry(update_event.logId).or_default() += update_event.work;
        }
    }

    env::commit_slice(&journal.abi_encode());
}
