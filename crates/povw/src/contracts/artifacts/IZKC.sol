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

// TODO(povw) Use IRewards and ZKC from the zkc repo directly.

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
