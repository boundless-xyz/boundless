// Copyright 2026 Boundless Foundation, Inc.
//
// Use of this source code is governed by the Business Source License
// as found in the LICENSE-BSL file.

use std::collections::{btree_map, BTreeMap};

use alloy_primitives::{Address, B256, U256};
use alloy_sol_types::SolValue;
use boundless_povw::log_updater::IPovwAccounting;
use boundless_povw::mint_calculator::{
    FixedPoint, Input, MintCalculatorJournal, MintCalculatorMint, MintCalculatorUpdate, CHAIN_SPECS,
};
use boundless_povw::zkc::{IZKCRewards, IZKC};
use risc0_steel::{Contract, Event};
use risc0_zkvm::guest::env;

/// A mapping from epoch number => { work log ID =>  { recipient => { reward weight } } }.
///
/// This mapping is structured to have all the information needed to apply the reward cap such that
/// within a single epoch, and a single work log, the sum of rewards accross all recipients is less
/// than or equal to the reward cap for the work log.
type RewardWeightMap = BTreeMap<U256, BTreeMap<Address, BTreeMap<Address, FixedPoint>>>;

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
    let input = Input::decode(env::read_frame()).expect("failed to deserialize input");

    // Converts the input into a `EvmEnv` structs for execution.
    let chain_spec = &CHAIN_SPECS.get(&input.chain_id).expect("unrecognized chain id in input");
    let envs = input.env.into_env(chain_spec);

    // Construct a mapping with the total work value for each finalized epoch.
    let mut latest_epoch_finalization_block: Option<u64> = None;
    let mut epochs = BTreeMap::<U256, U256>::new();
    for env in envs.0.values() {
        // Query all `EpochFinalized` events of the PoVW accounting contract.
        // NOTE: If it is a bottleneck, this can be optimized by taking a hint from the host as to
        // which blocks contain these events.
        let epoch_finalized_events = Event::new::<IPovwAccounting::EpochFinalized>(env)
            .address(input.povw_accounting_address)
            .query();

        // NOTE: This loop will iterate at most once when querying the real PovwAccounting impl.
        for epoch_finalized_event in epoch_finalized_events {
            let epoch_number = epoch_finalized_event.epoch;
            let None = epochs.insert(epoch_number, epoch_finalized_event.totalWork) else {
                panic!("multiple epoch finalized events for epoch {epoch_number}");
            };

            // Record the latest block number in which an epoch finalization event occurred.
            // NOTE: The BTreeMap iterator will visit the blocks in the env by increasing number.
            latest_epoch_finalization_block = Some(env.header().number);
        }
    }

    // Construct the mapping of calculated rewards, with the key as (epoch, recipient) pairs and
    // the value as a FixedPoint fraction indicating the portion of the PoVW epoch reward to assign
    let mut rewards_weights = RewardWeightMap::new();
    let mut updates = BTreeMap::<Address, (B256, B256)>::new();
    for env in envs.0.values() {
        // Query all `WorkLogUpdated` events of the PoVW accounting contract.
        // NOTE: If it is a bottleneck, this can be optimized by taking a hint from the host as to
        // which blocks contain these events.
        let update_events = Event::new::<IPovwAccounting::WorkLogUpdated>(env)
            .address(input.povw_accounting_address)
            .query();

        for update_event in update_events {
            // Check the work log ID filter to see if this event should be processed.
            if !input.work_log_filter.includes(update_event.workLogId.into()) {
                continue;
            }
            // Get the total work; skip this event if there is not an associated epoch finalization.
            // NOTE: This prevents events from e.g. the current unfinalized epoch from preventing
            // the mint. If this check causes a required update to be skipped, then the chaining
            // check or the completeness check below will fail.
            let Some(epoch_total_work) = epochs.get(&update_event.epochNumber).copied() else {
                continue;
            };

            // Insert or update the work log commitment for work log ID in the event.
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

            // Update mint value, skipping zero-valued updates.
            if update_event.updateValue > U256::ZERO {
                // NOTE: epoch_total_work must be greater than zero at this point, since it at
                // least contains this update, which has a non-zero value.
                *rewards_weights
                    .entry(update_event.epochNumber)
                    .or_default()
                    .entry(update_event.workLogId)
                    .or_default()
                    .entry(update_event.valueRecipient)
                    .or_default() +=
                    FixedPoint::fraction(update_event.updateValue, epoch_total_work);
            }
        }
    }

    // Ensure that for each work log, all epochs they participated in were processed fully. This
    // is important to avoid to ensure the correct application of the reward cap per epoch, which
    // is calculated below. Without this check, a prover could split their rewards for a single
    // epoch into multiple mints to avoid this restriction.
    //
    // To ensure this, we check that the work log commit recording on the PovwAccounting contract
    // at the end of the final epoch is equal to the final update provided for each work log. We
    // use the env for the block prior to the final epoch finalization event, which serves as a
    // snapshot of the final state of the PovwAccounting contract at the end of that epoch.
    let latest_epoch_finalization_block = latest_epoch_finalization_block.unwrap();
    let completness_check_env = envs.0.get(&(latest_epoch_finalization_block - 1)).unwrap();
    let povw_accounting_contract =
        Contract::new(input.povw_accounting_address, completness_check_env);
    for (work_log_id, (_, updated_commit)) in updates.iter() {
        let final_commit = povw_accounting_contract
            .call_builder(&IPovwAccounting::workLogCommitCall { workLogId: *work_log_id })
            .call();
        assert_eq!(
            final_commit,
            *updated_commit,
            "final commit at block {} does not match the updated commit: {final_commit} != {updated_commit}",
            completness_check_env.header().number
        );
    }

    // Calculate the rewards for each recipient by assigning the portion of each epoch rewards they
    // earned, capped by their max allowed reward in that epoch.
    let mut rewards = BTreeMap::<Address, U256>::new();
    // The calculator needs to query values from the chain state. This must be done from a block
    // that is later than the end of every epoch in the mint. We use the latest block in which an
    // epoch finalization occurred. By construction, this block must be after all epochs processed.
    let finalization_env = envs.0.get(&latest_epoch_finalization_block).unwrap();
    let zkc_contract = Contract::new(input.zkc_address, finalization_env);
    let zkc_rewards_contract = Contract::new(input.zkc_rewards_address, finalization_env);
    for (epoch, epoch_reward_weights) in rewards_weights {
        // Call the ZKC contract to get the total PoVW emissions and end time for the epoch.
        let epoch_emissions =
            zkc_contract.call_builder(&IZKC::getPoVWEmissionsForEpochCall { epoch }).call();
        let epoch_end_time = zkc_contract.call_builder(&IZKC::getEpochEndTimeCall { epoch }).call();

        for (work_log_id, work_log_reward_weights) in epoch_reward_weights {
            // Get the reward cap for this work log in the given epoch. Note that the reward cap is
            // determined at the end of the epoch.
            // NOTE: The reward cap is calculated from the work log ID such that the completness
            // check above will ensure all events for the epoch are included.
            let mut reward_cap = zkc_rewards_contract
                .call_builder(&IZKCRewards::getPastPoVWRewardCapCall {
                    account: work_log_id,
                    timepoint: epoch_end_time,
                })
                .call();

            // Iterate through the list of recipients for this work log, assigning rewards to each
            // and reducing the remaining cap each time.
            // If the work log's total rewards reach the cap, then rewards are assigned to
            // recipients based on the sorted order of their addresses. This ordering is considered
            // arbitrary, and may change in the future. In most cases we expect a work log to have
            // a single recipient.
            for (recipient, weight) in work_log_reward_weights {
                // Calculate the maximum reward, based on the povw value alone.
                let uncapped_reward = weight.mul_unwrap(epoch_emissions);

                // Apply the cap and add the reward to the final mapping.
                let reward = U256::min(uncapped_reward, reward_cap);
                if reward > U256::ZERO {
                    *rewards.entry(recipient).or_default() += reward;
                }

                reward_cap = reward_cap.saturating_sub(reward);
            }
        }
    }

    let journal = MintCalculatorJournal {
        mints: rewards
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
        zkcAddress: input.zkc_address,
        zkcRewardsAddress: input.zkc_rewards_address,
        povwAccountingAddress: input.povw_accounting_address,
        steelCommit: envs.commitment().unwrap().clone(),
    };
    env::commit_slice(&journal.abi_encode());
}
