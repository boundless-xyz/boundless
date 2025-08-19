// Copyright (c) 2025 RISC Zero, Inc.
//
// All rights reserved.

pragma solidity ^0.8.24;

import {ERC20} from "@openzeppelin/contracts/token/ERC20/ERC20.sol";
import {ERC20Permit} from "@openzeppelin/contracts/token/ERC20/extensions/ERC20Permit.sol";
import {IZKC, IZKCRewards} from "../src/povw/IZKC.sol";

contract MockZKC is IZKC, ERC20, ERC20Permit {
    uint256 public constant EPOCH_DURATION = 2 days;

    constructor() ERC20("Mock ZKC", "MOCK_ZKC") ERC20Permit("Mock ZKC") {}

    function mintPoVWRewardsForRecipient(address recipient, uint256 amount) external {
        _mint(recipient, amount);
    }

    function getPoVWEmissionsForEpoch(uint256) external view returns (uint256) {
        return 100 * 10 ** decimals();
    }

    /// Get the current epoch number for the ZKC system.
    ///
    /// The epoch number is guaranteed to be a monotonic increasing function, and is guaranteed to
    /// be stable withing a block.
    function getCurrentEpoch() external view returns (uint256) {
        return block.timestamp / EPOCH_DURATION;
    }

    // Returns the start time of the provided epoch.
    function getEpochStartTime(uint256 epoch) public pure returns (uint256) {
        return epoch * EPOCH_DURATION;
    }

    // Returns the end time of the provided epoch. Meaning the final timestamp
    // at which the epoch is "active". After this timestamp is finalized, the
    // state at this timestamp represents the final state of the epoch.
    function getEpochEndTime(uint256 epoch) public pure returns (uint256) {
        return getEpochStartTime(epoch + 1) - 1;
    }
}

contract MockZKCRewards is IZKCRewards {
    mapping(address => uint256) internal rewardsPovwPerEpochCap;

    function getPoVWRewardCap(address account) external view returns (uint256) {
        uint256 cap = rewardsPovwPerEpochCap[account];
        if (cap == 0) {
            return type(uint256).max;
        }
        return cap;
    }

    // This function only exists on the mock contract. Setting to 0 resets the cap to uint256 max.
    function setPoVWRewardCap(address account, uint256 cap) external {
        rewardsPovwPerEpochCap[account] = cap;
    }

    function getPastPoVWRewardCap(address, uint256) external pure returns (uint256) {
        revert("getPastPoVWRewardCap uimplemented");
    }
}
