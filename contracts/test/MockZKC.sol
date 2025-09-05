// Copyright 2025 RISC Zero, Inc.
//
// Use of this source code is governed by the Business Source License
// as found in the LICENSE-BSL file.

pragma solidity ^0.8.24;

import {ERC20} from "@openzeppelin/contracts/token/ERC20/ERC20.sol";
import {ERC20Permit} from "@openzeppelin/contracts/token/ERC20/extensions/ERC20Permit.sol";
import {IZKC, IZKCRewards} from "../src/zkc/IZKC.sol";

struct EpochEmissionsUpdate {
    uint256 epoch;
    uint256 emissions;
}

contract MockZKC is IZKC, ERC20, ERC20Permit {
    uint256 public constant EPOCH_DURATION = 2 days;

    EpochEmissionsUpdate[] internal epochEmissionsUpdates;

    constructor() ERC20("Mock ZKC", "MOCK_ZKC") ERC20Permit("Mock ZKC") {
        // When the contract is created, the emissions rate is initially set to 100.
        epochEmissionsUpdates.push(EpochEmissionsUpdate({epoch: 0, emissions: 100 * 10 ** decimals()}));
    }

    /// Get the current epoch number for the ZKC system.
    ///
    /// The epoch number is guaranteed to be a monotonic increasing function, and is guaranteed to
    /// be stable withing a block.
    function getCurrentEpoch() public view returns (uint256) {
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

    // This function only exists on the mock contract.
    function setPoVWEmissionsPerEpoch(uint256 emissions) external {
        epochEmissionsUpdates.push(EpochEmissionsUpdate({epoch: getCurrentEpoch(), emissions: emissions}));
    }

    function getPoVWEmissionsForEpoch(uint256 epoch) external view returns (uint256) {
        require(epoch < getCurrentEpoch(), "epoch must be past");

        for (uint256 i = 0; i < epochEmissionsUpdates.length; i++) {
            EpochEmissionsUpdate storage update = epochEmissionsUpdates[i];
            if (update.epoch < getCurrentEpoch()) {
                return update.emissions;
            }
        }
        revert("unreachable");
    }

    function mintPoVWRewardsForRecipient(address recipient, uint256 amount) external {
        _mint(recipient, amount);
    }

    function mintStakingRewardsForRecipient(address recipient, uint256 amount) external {
        _mint(recipient, amount);
    }
}

struct RewardsCapUpdate {
    uint256 timepoint;
    uint256 cap;
}

contract MockZKCRewards is IZKCRewards {
    mapping(address => RewardsCapUpdate[]) internal rewardsPovwPerEpochCapUpdates;

    // This function only exists on the mock contract. Setting to 0 resets the cap to uint256 max.
    function setPoVWRewardCap(address account, uint256 cap) external {
        rewardsPovwPerEpochCapUpdates[account].push(RewardsCapUpdate({timepoint: block.timestamp, cap: cap}));
    }

    function getPoVWRewardCap(address account) external view returns (uint256) {
        return getPastPoVWRewardCap(account, block.timestamp);
    }

    function getPastPoVWRewardCap(address account, uint256 timepoint) public view returns (uint256) {
        require(timepoint <= block.timestamp, "timepoint must be less than current timestamp");

        RewardsCapUpdate[] storage updates = rewardsPovwPerEpochCapUpdates[account];
        // No cap has been set for the given account.
        if (updates.length == 0) {
            return type(uint256).max;
        }
        for (uint256 i = 0; i < updates.length; i++) {
            if (updates[i].timepoint <= block.timestamp) {
                return updates[i].cap;
            }
        }
        revert("unreachable");
    }
}
