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

use super::common;

#[tokio::test]
#[ignore = "Requires ETH_MAINNET_RPC_URL"]
async fn test_vote_delegations_by_epoch() {
    let db = common::setup_test_db().await;

    // Test vote delegations for epoch 3
    let delegations = db
        .get_vote_delegation_powers_by_epoch(3, 0, 5)
        .await
        .expect("Failed to get vote delegations for epoch 3");

    // We limit to 5 but there are many more delegations
    assert_eq!(delegations.len(), 5);

    // Check first delegation (staker delegates to themselves)
    let first = &delegations[0];
    assert_eq!(
        format!("{:#x}", first.delegate_address),
        "0x2408e37489c231f883126c87e8aadbad782a040a"
    );
    assert_eq!(first.vote_power.to_string(), "726927981342423248000000");
    assert_eq!(first.delegator_count, 0); // Self-delegation not counted

    // Check second delegation
    let second = &delegations[1];
    assert_eq!(
        format!("{:#x}", second.delegate_address),
        "0x7cc3376b8d38b2c923cd9d5164f9d74e303482b2"
    );
    assert_eq!(second.vote_power.to_string(), "603060340000000000000000");
    assert_eq!(second.delegator_count, 0);
}

#[tokio::test]
#[ignore = "Requires ETH_MAINNET_RPC_URL"]
async fn test_reward_delegations_by_epoch() {
    let db = common::setup_test_db().await;

    // Test reward delegations for epoch 3
    let delegations = db
        .get_reward_delegation_powers_by_epoch(3, 0, 5)
        .await
        .expect("Failed to get reward delegations for epoch 3");

    // We limit to 5 but there are many delegations
    assert_eq!(delegations.len(), 5);

    // Check first delegation (receives delegated rewards)
    let first = &delegations[0];
    assert_eq!(
        format!("{:#x}", first.delegate_address),
        "0x0164ec96442196a02931f57e7e20fa59cff43845"
    );
    assert_eq!(first.reward_power.to_string(), "726927981342423248000000");
    assert_eq!(first.delegator_count, 1); // Has one delegator

    // Check that the delegator is the expected address
    assert_eq!(first.delegators.len(), 1);
    assert_eq!(format!("{:#x}", first.delegators[0]), "0x2408e37489c231f883126c87e8aadbad782a040a");

    // Check second (self-delegation)
    let second = &delegations[1];
    assert_eq!(
        format!("{:#x}", second.delegate_address),
        "0x7cc3376b8d38b2c923cd9d5164f9d74e303482b2"
    );
    assert_eq!(second.reward_power.to_string(), "603060340000000000000000");
    assert_eq!(second.delegator_count, 0); // Self-delegation
}

#[tokio::test]
#[ignore = "Requires ETH_MAINNET_RPC_URL"]
async fn test_vote_delegation_aggregates() {
    let db = common::setup_test_db().await;

    let aggregates = db
        .get_vote_delegation_powers_aggregate(0, 5)
        .await
        .expect("Failed to get vote delegation aggregates");

    // We limit to 5 but there are many more delegations
    assert_eq!(aggregates.len(), 5);

    // Check aggregate vote power
    let first = &aggregates[0];
    assert_eq!(
        format!("{:#x}", first.delegate_address),
        "0x2408e37489c231f883126c87e8aadbad782a040a"
    );
    assert_eq!(first.total_vote_power.to_string(), "726927981342423248000000");
    assert_eq!(first.epochs_participated, 3); // Participated in epochs 2, 3, 4
}

#[tokio::test]
#[ignore = "Requires ETH_MAINNET_RPC_URL"]
async fn test_reward_delegation_aggregates() {
    let db = common::setup_test_db().await;

    let aggregates = db
        .get_reward_delegation_powers_aggregate(0, 5)
        .await
        .expect("Failed to get reward delegation aggregates");

    // We limit to 5 but there are many delegations
    assert_eq!(aggregates.len(), 5);

    // Check aggregate reward power for the delegate
    let first = &aggregates[0];
    assert_eq!(
        format!("{:#x}", first.delegate_address),
        "0x0164ec96442196a02931f57e7e20fa59cff43845"
    );
    assert_eq!(first.total_reward_power.to_string(), "726927981342423248000000");
    assert_eq!(first.delegator_count, 1);
    assert_eq!(first.epochs_participated, 3); // Delegated in epochs 2, 3, 4
}
