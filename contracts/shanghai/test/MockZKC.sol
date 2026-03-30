// Copyright 2026 Boundless Foundation, Inc.
//
// Use of this source code is governed by the Business Source License
// as found in the LICENSE-BSL file.

pragma solidity ^0.8.26;

import {ERC20} from "@openzeppelin/contracts/token/ERC20/ERC20.sol";
import {ERC20Permit} from "@openzeppelin/contracts/token/ERC20/extensions/ERC20Permit.sol";
import {IZKC} from "zkc/interfaces/IZKC.sol";
import {IRewards as IZKCRewards} from "zkc/interfaces/IRewards.sol";

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
    // forge-lint: disable-next-item(mixed-case-function)
    function setPoVWEmissionsPerEpoch(uint256 emissions) external {
        epochEmissionsUpdates.push(EpochEmissionsUpdate({epoch: getCurrentEpoch(), emissions: emissions}));
    }

    // forge-lint: disable-next-item(mixed-case-function)
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

    // forge-lint: disable-next-item(mixed-case-function)
    function mintPoVWRewardsForRecipient(address recipient, uint256 amount) external {
        _mint(recipient, amount);
    }

    function mintStakingRewardsForRecipient(address recipient, uint256 amount) external {
        _mint(recipient, amount);
    }

    function claimedTotalSupply() external pure returns (uint256) {
        revert("not implemented");
    }

    function getCurrentEpochEndTime() external pure returns (uint256) {
        revert("not implemented");
    }

    function getEmissionsForEpoch(uint256 epoch) external pure returns (uint256) {
        epoch;
        revert("not implemented");
    }

    function getStakingEmissionsForEpoch(uint256 epoch) external pure returns (uint256) {
        epoch;
        revert("not implemented");
    }

    function getSupplyAtEpochStart(uint256 epoch) external pure returns (uint256) {
        epoch;
        revert("not implemented");
    }

    // forge-lint: disable-next-item(mixed-case-function)
    function getTotalPoVWEmissionsAtEpochStart(uint256 epoch) external pure returns (uint256) {
        epoch;
        revert("not implemented");
    }

    function getTotalStakingEmissionsAtEpochStart(uint256 epoch) external pure returns (uint256) {
        epoch;
        revert("not implemented");
    }

    function initialMint(address[] calldata recipients, uint256[] calldata amounts) external pure {
        recipients;
        amounts;
        revert("not implemented");
    }
}

struct RewardsCapUpdate {
    uint256 timepoint;
    uint256 cap;
}

contract MockZKCRewards is IZKCRewards {
    mapping(address => RewardsCapUpdate[]) internal rewardsPovwPerEpochCapUpdates;

    // This function only exists on the mock contract. Setting to 0 resets the cap to uint256 max.
    // forge-lint: disable-next-item(mixed-case-function)
    function setPoVWRewardCap(address account, uint256 cap) external {
        rewardsPovwPerEpochCapUpdates[account].push(RewardsCapUpdate({timepoint: block.timestamp, cap: cap}));
    }

    // forge-lint: disable-next-item(mixed-case-function)
    function getPoVWRewardCap(address account) external view returns (uint256) {
        return getPastPoVWRewardCap(account, block.timestamp);
    }

    // forge-lint: disable-next-item(mixed-case-function)
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

    function delegateRewards(address delegatee) external pure {
        delegatee;
        revert("not implemented");
    }

    function delegateRewardsBySig(address delegatee, uint256 nonce, uint256 expiry, uint8 v, bytes32 r, bytes32 s)
        external
        pure
    {
        delegatee;
        nonce;
        expiry;
        v;
        r;
        s;
        revert("not implemented");
    }

    function getPastStakingRewards(address account, uint256 timepoint) external pure returns (uint256) {
        account;
        timepoint;
        revert("not implemented");
    }

    function getPastTotalStakingRewards(uint256 timepoint) external pure returns (uint256) {
        timepoint;
        revert("not implemented");
    }

    function getStakingRewards(address account) external pure returns (uint256) {
        account;
        revert("not implemented");
    }

    function getTotalStakingRewards() external pure returns (uint256) {
        revert("not implemented");
    }

    function rewardDelegates(address account) external pure returns (address) {
        account;
        revert("not implemented");
    }
}
