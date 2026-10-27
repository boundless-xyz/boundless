// SPDX-License-Identifier: MIT
pragma solidity 0.8.26;

/// @title IZKC
/// @notice Interface for the ZKC token with epoch-based emissions
/// @dev Defines ZKC-specific functionality for epoch-based reward distribution
interface IZKC {
    /// @notice Emitted when a recipient claims PoVW rewards.
    /// @param recipient The address that claimed the rewards
    /// @param amount The amount of ZKC tokens claimed
    /// @dev The reward amount could include ZKC that was earned across multiple epochs.
    event PoVWRewardsClaimed(address indexed recipient, uint256 amount);

    /// @notice Emitted when a recipient claims staking rewards.
    /// @param recipient The address that claimed the rewards
    /// @param amount The amount of ZKC tokens claimed
    /// @dev The reward amount could include ZKC that was earned across multiple epochs.
    event StakingRewardsClaimed(address indexed recipient, uint256 amount);

    error EpochNotEnded(uint256 epoch);
    error TotalAllocationExceeded();
    error EpochsNotStarted();

    /// @notice Perform initial token distribution to specified recipients
    /// @dev Only callable by designated initial minters
    /// @param recipients Array of addresses to receive tokens
    /// @param amounts Array of token amounts corresponding to each recipient
    function initialMint(address[] calldata recipients, uint256[] calldata amounts) external;

    /// @notice Mint PoVW rewards for a specific recipient
    /// @dev Only callable by addresses with POVW_MINTER_ROLE
    /// @param recipient Address to receive the minted rewards
    /// @param amount Amount of tokens to mint
    function mintPoVWRewardsForRecipient(address recipient, uint256 amount) external;

    /// @notice Mint staking rewards for a specific recipient
    /// @dev Only callable by addresses with STAKING_MINTER_ROLE
    /// @param recipient Address to receive the minted rewards
    /// @param amount Amount of tokens to mint
    function mintStakingRewardsForRecipient(address recipient, uint256 amount) external;

    /// @notice Get the total supply at the start of a specific epoch
    /// @dev ZKC is emitted at the end of each epoch, so this excludes rewards generated
    ///      as part of staking/work during the current epoch.
    /// @param epoch The epoch number (0-indexed)
    /// @return The total supply at the start of the epoch
    function getSupplyAtEpochStart(uint256 epoch) external pure returns (uint256);

    /// @notice Get the cumulative total PoVW emissions since genesis up to the start of a specific epoch
    /// @param epoch The epoch number
    /// @return Total PoVW emissions up to the epoch start
    function getTotalPoVWEmissionsAtEpochStart(uint256 epoch) external returns (uint256);

    /// @notice Get the cumulative total staking emissions since genesis up to the start of a specific epoch
    /// @param epoch The epoch number
    /// @return Total staking emissions up to the epoch start
    function getTotalStakingEmissionsAtEpochStart(uint256 epoch) external returns (uint256);

    /// @notice Get the total ZKC that will be emitted at the _end_ of the specified epoch
    /// @dev Includes both PoVW and staking rewards
    /// @param epoch The epoch number
    /// @return Total emissions for the epoch
    function getEmissionsForEpoch(uint256 epoch) external returns (uint256);

    /// @notice Get the PoVW emissions that will be emitted at the _end_ of the specified epoch
    /// @param epoch The epoch number
    /// @return PoVW emissions for the epoch
    function getPoVWEmissionsForEpoch(uint256 epoch) external returns (uint256);

    /// @notice Get the staking emissions that will be emitted at the _end_ of the specified epoch
    /// @param epoch The epoch number
    /// @return Staking emissions for the epoch
    function getStakingEmissionsForEpoch(uint256 epoch) external returns (uint256);

    /// @notice Get the current epoch number
    /// @dev Calculated based on time elapsed since deployment.
    /// @dev Reverts if epochs have not started yet.
    /// @return The current epoch number (0-indexed)
    function getCurrentEpoch() external view returns (uint256);

    /// @notice Get the current epoch end time
    /// @dev Returns the final timestamp at which the current epoch is active.
    ///      After this time, rewards will be emitted.
    /// @return The timestamp when the current epoch ends
    function getCurrentEpochEndTime() external view returns (uint256);

    /// @notice Get the start timestamp of a specific epoch
    /// @dev Reverts if epochs have not started yet.
    /// @param epoch The epoch number
    /// @return The timestamp when the epoch starts
    function getEpochStartTime(uint256 epoch) external view returns (uint256);

    /// @notice Get the end timestamp of a specific epoch
    /// @dev Returns the final timestamp at which the epoch is active
    /// @dev Reverts if epochs have not started yet.
    /// @param epoch The epoch number
    /// @return The timestamp when the epoch ends
    function getEpochEndTime(uint256 epoch) external view returns (uint256);

    /// @notice Get the actual minted and claimed total supply
    /// @dev This represents the initial supply that was minted and allocated to initial minters,
    ///      as well as tokens that have been claimed (and thus minted) via PoVW or Staking rewards.
    /// @return The total amount of tokens that have been claimed
    function claimedTotalSupply() external view returns (uint256);
}
