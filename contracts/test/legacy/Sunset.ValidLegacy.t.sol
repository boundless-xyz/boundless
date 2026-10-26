// Copyright 2026 Boundless Foundation, Inc.
// SPDX-License-Identifier: BUSL-1.1
pragma solidity ^0.8.26;
import {BoundlessMarketLegacyViaFallbackBasicTest} from "./BoundlessMarketLegacyViaFallback.t.sol";

/// Reuse the existing legacy signed-request and real fallback settlement assertions at T-1.
contract LegacyMarketValidSunsetTest is BoundlessMarketLegacyViaFallbackBasicTest {
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
