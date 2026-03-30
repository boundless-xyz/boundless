// Copyright 2026 Boundless Foundation, Inc.
//
// Use of this source code is governed by the Business Source License
// as found in the LICENSE-BSL file.
// SPDX-License-Identifier: BUSL-1.1

pragma solidity ^0.8.26;

import {Initializable} from "@openzeppelin/contracts-upgradeable/proxy/utils/Initializable.sol";
import {Ownable2StepUpgradeable} from "@openzeppelin/contracts-upgradeable/access/Ownable2StepUpgradeable.sol";
import {UUPSUpgradeable} from "@openzeppelin/contracts-upgradeable/proxy/utils/UUPSUpgradeable.sol";
import {Strings} from "@openzeppelin/contracts/utils/Strings.sol";
import {IVersionRegistry} from "./IVersionRegistry.sol";

/// @title VersionRegistry
/// @notice On-chain registry storing the minimum acceptable broker version.
///         The broker reads this contract on startup and every 10 minutes.
///         If the broker version is below the minimum, the broker shuts down.
/// @dev Deployed behind an ERC1967Proxy (UUPS pattern). Owned by governance multisig.
///      No downgrade protection: the minimum can be lowered to allow rollback.
contract VersionRegistry is IVersionRegistry, Initializable, Ownable2StepUpgradeable, UUPSUpgradeable {
    /// @dev The version of the contract, with respect to upgrades.
    uint64 public constant VERSION = 1;

    /// @notice The minimum acceptable broker version, encoded as (major << 32) | (minor << 16) | patch.
    uint64 public minimumBrokerVersion;

    /// @notice Governance notice displayed to brokers on each version check. Empty means no notice.
    string public notice;

    /// @custom:oz-upgrades-unsafe-allow constructor
    constructor() {
        _disableInitializers();
    }

    /// @notice Initialize the contract and set the owner.
    /// @param _owner The initial owner (governance multisig).
    function initialize(address _owner) external initializer {
        __Ownable2Step_init();
        __UUPSUpgradeable_init();
        _transferOwnership(_owner);
    }

    /// @inheritdoc UUPSUpgradeable
    function _authorizeUpgrade(address) internal override onlyOwner {}

    /// @inheritdoc IVersionRegistry
    function setMinimumBrokerVersion(uint64 _version) external onlyOwner {
        _setMinimumBrokerVersion(_version);
    }

    /// @inheritdoc IVersionRegistry
    function setMinimumBrokerVersionSemver(uint16 major, uint16 minor, uint16 patch) external onlyOwner {
        _setMinimumBrokerVersion(packVersion(major, minor, patch));
    }

    /// @inheritdoc IVersionRegistry
    function minimumBrokerVersionSemver() external view returns (uint16 major, uint16 minor, uint16 patch) {
        return unpackVersion(minimumBrokerVersion);
    }

    /// @inheritdoc IVersionRegistry
    function packVersion(uint16 major, uint16 minor, uint16 patch) public pure returns (uint64) {
        return (uint64(major) << 32) | (uint64(minor) << 16) | uint64(patch);
    }

    /// @inheritdoc IVersionRegistry
    function unpackVersion(uint64 _version) public pure returns (uint16 major, uint16 minor, uint16 patch) {
        major = uint16(_version >> 32);
        minor = uint16(_version >> 16);
        patch = uint16(_version);
    }

    /// @inheritdoc IVersionRegistry
    function minimumBrokerVersionString() external view returns (string memory) {
        (uint16 major, uint16 minor, uint16 patch) = unpackVersion(minimumBrokerVersion);
        return string.concat(Strings.toString(major), ".", Strings.toString(minor), ".", Strings.toString(patch));
    }

    function _setMinimumBrokerVersion(uint64 _version) internal {
        if (_version > type(uint48).max) revert InvalidVersion();
        uint64 old = minimumBrokerVersion;
        minimumBrokerVersion = _version;
        emit MinimumBrokerVersionUpdated(old, _version);
    }

    /// @inheritdoc IVersionRegistry
    function setNotice(string calldata _notice) external onlyOwner {
        string memory old = notice;
        notice = _notice;
        emit NoticeUpdated(old, _notice);
    }

    /// @inheritdoc IVersionRegistry
    function getVersionInfo() external view returns (uint64 minimumVersion, string memory _notice) {
        return (minimumBrokerVersion, notice);
    }
}
