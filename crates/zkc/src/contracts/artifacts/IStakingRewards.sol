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

pragma solidity ^0.8.26;

/// @title StakingRewards
/// @notice Contract for distributing staking rewards based on veZKC staking positions
/// @dev Users can claim rewards for specific epochs based on their staking value
interface IStakingRewards {
    /// @notice Claim rewards for the given epochs
    /// @param epochs The epochs to claim rewards for
    /// @return amount The amount of rewards claimed
    function claimRewards(uint256[] calldata epochs) external returns (uint256 amount);

    /// @notice Claim rewards for the given epochs and send to a recipient
    /// @param epochs The epochs to claim rewards for
    /// @param recipient The address to receive the minted rewards
    /// @return amount The amount of rewards claimed
    function claimRewardsToRecipient(uint256[] calldata epochs, address recipient) external returns (uint256);

    /// @notice Calculate the rewards a user is owed for the given epochs. If the epoch has not ended yet, it will return zero rewards.
    /// @param user The user address
    /// @param epochs The epochs to calculate rewards for
    /// @return rewards The rewards owed
    function calculateRewards(address user, uint256[] calldata epochs) external returns (uint256[] memory);

    /// @notice Calculate unclaimed rewards for a user - returns 0 for already claimed epochs
    /// @param user The user address
    /// @param epochs The epochs to calculate unclaimed rewards for
    /// @return rewards The unclaimed rewards (0 if already claimed)
    function calculateUnclaimedRewards(address user, uint256[] calldata epochs) external returns (uint256[] memory);

    /// @notice Check if a user has claimed rewards for a specific epoch
    /// @param user The user address
    /// @param epoch The epoch to check
    /// @return claimed Whether rewards have been claimed
    function hasUserClaimedRewards(address user, uint256 epoch) external view returns (bool claimed);

    /// @notice Get the current epoch from the ZKC contract
    /// @return currentEpoch The current epoch number
    function getCurrentEpoch() external view returns (uint256 currentEpoch);
}
