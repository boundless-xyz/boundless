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

// TODO(povw) Use IRewards from the zkc repo directly.

pragma solidity ^0.8.20;

/// @title IRewards
/// @notice Interface for reward distribution calculations
/// @dev Used by external contracts to determine reward allocations based on stake amounts
interface IRewards {
    /// @notice Get current staking rewards power for an account
    /// @param account Account to query
    /// @return Reward power (staked amount / REWARD_POWER_SCALAR)
    function getStakingRewards(address account) external view returns (uint256);

    /// @notice Get historical staking rewards power for an account
    /// @param account Account to query
    /// @param timepoint Historical timestamp to query
    /// @return Reward power at the specified timestamp
    function getPastStakingRewards(address account, uint256 timepoint) external view returns (uint256);

    /// @notice Get total staking rewards power across all users
    /// @return Total reward power
    function getTotalStakingRewards() external view returns (uint256);

    /// @notice Get historical total staking rewards power
    /// @param timepoint Historical timestamp to query
    /// @return Total reward power at the specified timestamp
    function getPastTotalStakingRewards(uint256 timepoint) external view returns (uint256);

    /// @notice Get current PoVW reward cap for an account
    /// @param account Account to query
    /// @return PoVW reward cap (staked amount / POVW_REWARD_CAP_SCALAR)
    function getPoVWRewardCap(address account) external view returns (uint256);

    /// @notice Get historical PoVW reward cap for an account
    /// @param account Account to query
    /// @param timepoint Historical timestamp to query
    /// @return PoVW reward cap at the specified timestamp
    function getPastPoVWRewardCap(address account, uint256 timepoint) external view returns (uint256);

    /// @notice Returns the reward delegate chosen by an account
    /// @param account Account to query
    /// @return The address that account has delegated rewards to (or account itself if none)
    function rewardDelegates(address account) external view returns (address);

    /// @notice Delegate reward power to another address
    /// @param delegatee Address to delegate rewards to
    function delegateRewards(address delegatee) external;

    /// @notice Delegate rewards using a signature
    /// @param delegatee Address to delegate rewards to
    /// @param nonce Nonce for the signature
    /// @param expiry Expiration timestamp for the signature
    /// @param v Recovery byte of the signature
    /// @param r R component of the signature
    /// @param s S component of the signature
    function delegateRewardsBySig(address delegatee, uint256 nonce, uint256 expiry, uint8 v, bytes32 r, bytes32 s)
        external;
}
