// Copyright 2026 Boundless Foundation, Inc.
// SPDX-License-Identifier: BUSL-1.1
pragma solidity ^0.8.26;
import {BoundlessMarketBasicTest} from "../BoundlessMarket.t.sol";

/// Reuse the existing signed-request, nonempty-proof and settlement assertions at T-1.
contract CurrentMarketValidSunsetTest is BoundlessMarketBasicTest {
    function testSunsetValidLock() public {
        vm.warp(1793836799);
        testLockRequest();
    }

    function testSunsetValidSignedLock() public {
        vm.warp(1793836799);
        testLockRequestWithSignature();
    }

    function testSunsetValidFulfillment() public {
        vm.warp(1793836799);
        testFulfillLockedRequest();
    }

    function testSunsetValidRootAndFulfillment() public {
        vm.warp(1793836799);
        testSubmitRootAndFulfillLockedRequest();
    }

    function testSunsetValidOpenFulfillment() public {
        vm.warp(1793836799);
        testFulfillNeverLocked();
    }
}
