// Copyright (c) 2025 RISC Zero, Inc.
//
// All rights reserved.

pragma solidity ^0.8.24;

// TODO(povw) Use IRewards and ZKC from the ZKC directly.

/// A subset of the functionality implemented by IRewards in the ZKC repo. This is the subset used
/// by the PoVW rewards flow, and is copied here as the ZKC repo is not yet public.
interface IZKCRewards {
    function getPoVWRewardCap(address account) external view returns (uint256);
    function getPastPoVWRewardCap(address account, uint256 timepoint) external view returns (uint256);
}

/// A subset of the functionality implemented by the ZKC contract. This is the subset used
/// by the PoVW rewards flow, and is copied here as the ZKC repo is not yet public.
interface IZKC {
    function mintPoVWRewardsForRecipient(address recipient, uint256 amount) external;
    function getPoVWEmissionsForEpoch(uint256 epoch) external returns (uint256);
    function getEpochEndTime(uint256 epoch) external view returns (uint256);
    /// Get the current epoch number for the ZKC system.
    ///
    /// The epoch number is guaranteed to be a monotonic increasing function, and is guaranteed to
    /// be stable withing a block.
    function getCurrentEpoch() external view returns (uint256);
}
