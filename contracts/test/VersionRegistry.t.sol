// Copyright 2026 Boundless Foundation, Inc.
//
// Use of this source code is governed by the Business Source License
// as found in the LICENSE-BSL file.
// SPDX-License-Identifier: BUSL-1.1

pragma solidity ^0.8.26;

import {Test} from "forge-std/Test.sol";
import {UnsafeUpgrades} from "openzeppelin-foundry-upgrades/Upgrades.sol";
import {OwnableUpgradeable} from "@openzeppelin/contracts-upgradeable/access/OwnableUpgradeable.sol";
import {IVersionRegistry} from "../src/IVersionRegistry.sol";
import {VersionRegistry} from "../src/VersionRegistry.sol";

contract VersionRegistryTest is Test {
    VersionRegistry internal registry;
    address internal owner;
    address internal nonOwner;

    function setUp() public {
        owner = makeAddr("owner");
        nonOwner = makeAddr("nonOwner");

        address impl = address(new VersionRegistry());
        address proxy = UnsafeUpgrades.deployUUPSProxy(impl, abi.encodeCall(VersionRegistry.initialize, (owner)));
        registry = VersionRegistry(proxy);
    }

    function test_initialize() public view {
        assertEq(registry.owner(), owner);
        assertEq(registry.minimumBrokerVersion(), 0);
        assertEq(registry.minimumBrokerVersionString(), "0.0.0");

        assertEq(registry.VERSION(), 1);
    }

    function test_setMinimumBrokerVersion() public {
        uint64 version = 0x0000000200030001; // 2.3.1 packed
        vm.expectEmit(true, true, true, true);
        emit IVersionRegistry.MinimumBrokerVersionUpdated(0, version);

        vm.prank(owner);
        registry.setMinimumBrokerVersion(version);

        assertEq(registry.minimumBrokerVersion(), version);
    }

    function test_setMinimumBrokerVersion_notOwner() public {
        vm.prank(nonOwner);
        vm.expectRevert(abi.encodeWithSelector(OwnableUpgradeable.OwnableUnauthorizedAccount.selector, nonOwner));
        registry.setMinimumBrokerVersion(1);
    }

    function test_setMinimumBrokerVersion_invalidVersion() public {
        uint64 tooBig = uint64(type(uint48).max) + 1;
        vm.prank(owner);
        vm.expectRevert(abi.encodeWithSelector(IVersionRegistry.InvalidVersion.selector));
        registry.setMinimumBrokerVersion(tooBig);
    }

    function test_setMinimumBrokerVersionSemver() public {
        // (2 << 32) | (3 << 16) | 1 = 8589934592 + 196608 + 1 = 8590131201
        // Let's verify: 2 * 2^32 = 8589934592, 3 * 2^16 = 196608, patch = 1 => 8590131201
        uint64 expected = (uint64(2) << 32) | (uint64(3) << 16) | uint64(1);

        vm.expectEmit(true, true, true, true);
        emit IVersionRegistry.MinimumBrokerVersionUpdated(0, expected);

        vm.prank(owner);
        registry.setMinimumBrokerVersionSemver(2, 3, 1);

        assertEq(registry.minimumBrokerVersion(), expected);
        assertEq(expected, 8590131201);
    }

    function test_packUnpackRoundtrip() public view {
        uint16[3][5] memory cases = [
            [uint16(0), uint16(0), uint16(0)],
            [uint16(1), uint16(0), uint16(0)],
            [uint16(0), uint16(1), uint16(0)],
            [uint16(0), uint16(0), uint16(1)],
            [uint16(65535), uint16(65535), uint16(65535)]
        ];

        for (uint256 i = 0; i < cases.length; i++) {
            uint16 major = cases[i][0];
            uint16 minor = cases[i][1];
            uint16 patch = cases[i][2];

            uint64 packed = registry.packVersion(major, minor, patch);
            (uint16 rMajor, uint16 rMinor, uint16 rPatch) = registry.unpackVersion(packed);

            assertEq(rMajor, major);
            assertEq(rMinor, minor);
            assertEq(rPatch, patch);
        }
    }

    function test_minimumBrokerVersionString() public {
        // Test "0.0.0" before setting
        assertEq(registry.minimumBrokerVersionString(), "0.0.0");

        // Set to 2.3.1 and check string
        vm.prank(owner);
        registry.setMinimumBrokerVersionSemver(2, 3, 1);
        assertEq(registry.minimumBrokerVersionString(), "2.3.1");
    }

    function test_minimumBrokerVersionSemver() public {
        uint64 packed = registry.packVersion(5, 12, 99);
        vm.prank(owner);
        registry.setMinimumBrokerVersion(packed);

        (uint16 major, uint16 minor, uint16 patch) = registry.minimumBrokerVersionSemver();
        assertEq(major, 5);
        assertEq(minor, 12);
        assertEq(patch, 99);
    }

    function test_downgradeAllowed() public {
        vm.startPrank(owner);
        registry.setMinimumBrokerVersionSemver(2, 0, 0);
        assertEq(registry.minimumBrokerVersion(), registry.packVersion(2, 0, 0));

        // Downgrade to 1.0.0 — should not revert
        registry.setMinimumBrokerVersionSemver(1, 0, 0);
        assertEq(registry.minimumBrokerVersion(), registry.packVersion(1, 0, 0));

        // Downgrade further to 0.0.0 — should not revert
        registry.setMinimumBrokerVersion(0);
        assertEq(registry.minimumBrokerVersion(), 0);

        vm.stopPrank();
    }

    function test_setMinimumBrokerVersion_maxValid() public {
        // type(uint48).max == packVersion(65535, 65535, 65535) — should succeed (boundary is inclusive)
        uint64 maxValid = uint64(type(uint48).max);
        vm.prank(owner);
        registry.setMinimumBrokerVersion(maxValid);
        assertEq(registry.minimumBrokerVersion(), maxValid);
    }

    function test_setMinimumBrokerVersion_uint64Max() public {
        vm.prank(owner);
        vm.expectRevert(abi.encodeWithSelector(IVersionRegistry.InvalidVersion.selector));
        registry.setMinimumBrokerVersion(type(uint64).max);
    }

    function test_semverSetterCannotOverflow() public view {
        // The semver setter with max uint16 components produces exactly type(uint48).max
        uint64 packed = registry.packVersion(65535, 65535, 65535);
        assertEq(packed, uint64(type(uint48).max));
    }

    function test_consecutiveUpdates_eventTracksOldVersion() public {
        uint64 v1 = registry.packVersion(1, 0, 0);
        uint64 v2 = registry.packVersion(2, 0, 0);

        vm.startPrank(owner);

        vm.expectEmit(true, true, true, true);
        emit IVersionRegistry.MinimumBrokerVersionUpdated(0, v1);
        registry.setMinimumBrokerVersion(v1);

        // oldVersion in the second event must equal v1
        vm.expectEmit(true, true, true, true);
        emit IVersionRegistry.MinimumBrokerVersionUpdated(v1, v2);
        registry.setMinimumBrokerVersion(v2);

        vm.stopPrank();
    }

    function test_ownershipTransfer_twoStep() public {
        address newOwner = makeAddr("newOwner");

        vm.prank(owner);
        registry.transferOwnership(newOwner);

        // Owner hasn't changed yet
        assertEq(registry.owner(), owner);
        assertEq(registry.pendingOwner(), newOwner);

        // New owner accepts
        vm.prank(newOwner);
        registry.acceptOwnership();

        assertEq(registry.owner(), newOwner);
    }
}
