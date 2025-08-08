use std::collections::{btree_map, BTreeMap};
use std::sync::LazyLock;

use alloy_chains::NamedChain;
use alloy_primitives::{Address, ChainId, B256, U256};
use alloy_sol_types::SolValue;
use boundless_povw_guests::log_updater::IPovwAccounting;
use boundless_povw_guests::mint_calculator::{
    FixedPoint, Input, MintCalculatorJournal, MintCalculatorMint, MintCalculatorUpdate,
};
use risc0_steel::ethereum::{
    EthChainSpec, ANVIL_CHAIN_SPEC, ETH_MAINNET_CHAIN_SPEC, ETH_SEPOLIA_CHAIN_SPEC,
};
use risc0_steel::Event;
use risc0_zkvm::guest::env;

static CHAIN_SPECS: LazyLock<BTreeMap<ChainId, EthChainSpec>> = LazyLock::new(|| {
    BTreeMap::from([
        (NamedChain::Mainnet as ChainId, ETH_MAINNET_CHAIN_SPEC.clone()),
        (NamedChain::Sepolia as ChainId, ETH_SEPOLIA_CHAIN_SPEC.clone()),
        (NamedChain::AnvilHardhat as ChainId, ANVIL_CHAIN_SPEC.clone()),
    ])
});

// The mint calculator ensures:
// * An event was logged by the PoVW accounting contract for each log update and epoch finalization.
//   * Each event is counted at most once.
//   * Events form an unbroken chain from initialCommit to updatedCommit. This constitutes an
//     exhaustiveness check such that the prover cannot exclude updates, and thereby deny a reward.
// * Mint value is calculated correctly from the PoVW accounting totals in each included epoch.
//   * An event was logged by the PoVW accounting contract for epoch finalization.
//   * The total work from the epoch finalization event is used in the mint calculation.
//   * The mint recipient is set correctly.
fn main() {
    // Read the input from the guest environment.
    let input: Input =
        postcard::from_bytes(&env::read_frame()).expect("failed to deserialize input");

    // Converts the input into a `EvmEnv` structs for execution.
    let chain_spec = &CHAIN_SPECS.get(&input.chain_id).expect("unrecognized chain id in input");
    let envs = input.env.into_env(chain_spec);

    // Construct a mapping with the total work value for each finalized epoch.
    let mut epochs = BTreeMap::<u32, U256>::new();
    for env in envs.0.values() {
        // Query all `EpochFinalized` events of the PoVW accounting contract.
        // NOTE: If it is a bottleneck, this can be optimized by taking a hint from the host as to
        // which blocks contain these events.
        let epoch_finalized_events = Event::new::<IPovwAccounting::EpochFinalized>(env)
            .address(input.povw_contract_address)
            .query();

        for epoch_finalized_event in epoch_finalized_events {
            let epoch_number = epoch_finalized_event.epoch.to::<u32>();
            let None = epochs.insert(epoch_number, epoch_finalized_event.totalWork) else {
                panic!("multiple epoch finalized events for epoch {epoch_number}");
            };
        }
    }

    // Construct the mapping of mints, as recipient address to mint proportion pairs, and the
    // mapping of work log id to (initial commit, final commit) pairs.
    let mut mints = BTreeMap::<Address, FixedPoint>::new();
    let mut updates = BTreeMap::<Address, (B256, B256)>::new();
    for env in envs.0.values() {
        // Query all `WorkLogUpdated` events of the PoVW accounting contract.
        // NOTE: If it is a bottleneck, this can be optimized by taking a hint from the host as to
        // which blocks contain these events.
        let update_events = Event::new::<IPovwAccounting::WorkLogUpdated>(env)
            .address(input.povw_contract_address)
            .query();

        for update_event in update_events {
            match updates.entry(update_event.workLogId) {
                btree_map::Entry::Vacant(entry) => {
                    entry.insert((update_event.initialCommit, update_event.updatedCommit));
                }
                btree_map::Entry::Occupied(mut entry) => {
                    assert_eq!(
                        entry.get().1,
                        update_event.initialCommit,
                        "multiple update events for {:x} that do not form a chain",
                        update_event.workLogId
                    );
                    entry.get_mut().1 = update_event.updatedCommit;
                }
            }

            let epoch_number = update_event.epochNumber.to::<u32>();
            let epoch_total_work = *epochs.get(&epoch_number).unwrap_or_else(|| {
                panic!("no epoch finalized event processed for epoch number {epoch_number}")
            });
            // Update mint value, skipping zero-valued updates.
            if update_event.updateValue > U256::ZERO {
                // NOTE: epoch_total_work must be greater than zero at this point, since it at
                // least contains this update, which has a non-zero value.
                *mints.entry(update_event.valueRecipient).or_default() +=
                    FixedPoint::fraction(update_event.updateValue, epoch_total_work);
            }
        }
    }

    let journal = MintCalculatorJournal {
        mints: mints
            .into_iter()
            .map(|(recipient, value)| MintCalculatorMint { recipient, value })
            .collect(),
        updates: updates
            .into_iter()
            .map(|(log_id, commits)| MintCalculatorUpdate {
                workLogId: log_id,
                initialCommit: commits.0,
                updatedCommit: commits.1,
            })
            .collect(),
        povwContractAddress: input.povw_contract_address,
        steelCommit: envs.commitment().unwrap().clone(),
    };
    env::commit_slice(&journal.abi_encode());
}
