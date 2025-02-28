// Copyright (c) 2025 RISC Zero, Inc.
//
// All rights reserved.

pragma solidity ^0.8.20;

import "forge-std/Test.sol";
import {RequestLock, RequestLockLibrary} from "../../src/types/RequestLock.sol";

contract RequestLockTest is Test {
    using RequestLockLibrary for RequestLock;

    RequestLock requestLock;

    function setUp() public {
        requestLock = RequestLock({
            prover: address(0x123),
            lockDeadline: uint64(block.number + 100),
            deadlineDelta: uint24(50),
            requestLockFlags: 0,
            price: 1 ether,
            stake: 1 ether,
            fingerprint: bytes8(0x1234567890abcdef)
        });
    }

    function assertSlot2Clear() private view returns (uint256) {
        uint256 slot2;
        assembly {
            let s2 := add(requestLock.slot, 1)
            slot2 := sload(s2)
        }
        return slot2;
    }

    function testDeadline() public view {
        uint64 expectedDeadline = requestLock.lockDeadline + requestLock.deadlineDelta;
        assertEq(requestLock.deadline(), expectedDeadline, "Deadline calculation is incorrect");
    }

    function testSetProverPaidBeforeLockDeadline() public {
        requestLock.setProverPaidBeforeLockDeadline();
        assertEq(
            requestLock.requestLockFlags,
            RequestLockLibrary.PROVER_PAID_DURING_LOCK_FLAG,
            "Prover paid flag not set correctly"
        );
        assertEq(requestLock.price, 0, "Price not zeroed out");
        assertEq(requestLock.stake, 0, "Stake not zeroed out");
        assertEq(requestLock.fingerprint, bytes8(0), "Fingerprint not zeroed out");
        assertSlot2Clear();
    }

    function testSetProverPaidAfterLockDeadline() public {
        address prover = address(0x456);
        requestLock.setProverPaidAfterLockDeadline(prover);
        assertEq(requestLock.prover, prover);
        assertTrue(requestLock.isProverPaidAfterLockDeadline());
        assertFalse(requestLock.isProverPaidBeforeLockDeadline());
        assertFalse(requestLock.isSlashed());
    }

    function testSetSlashed() public {
        requestLock.setSlashed();
        assertEq(requestLock.requestLockFlags, RequestLockLibrary.SLASHED_FLAG, "Slashed flag not set correctly");
        assertEq(requestLock.price, 0, "Price not zeroed out");
        assertEq(requestLock.stake, 0, "Stake not zeroed out");
        assertEq(requestLock.fingerprint, bytes8(0), "Fingerprint not zeroed out");
        assertSlot2Clear();
    }

    function testIsProverPaidBeforeLockDeadline() public {
        requestLock.setProverPaidBeforeLockDeadline();
        assertTrue(requestLock.isProverPaidBeforeLockDeadline());
    }

    function testIsProverPaidAfterLockDeadline() public {
        requestLock.setProverPaidAfterLockDeadline(address(0x456));
        assertTrue(requestLock.isProverPaidAfterLockDeadline());
    }

    function testIsProverPaid() public {
        requestLock.setProverPaidBeforeLockDeadline();
        assertTrue(requestLock.isProverPaid());
    }

    function testIsProverPaid2() public {
        requestLock.setProverPaidAfterLockDeadline(address(0x456));
        assertTrue(requestLock.isProverPaid());
    }

    function testIsSlashed() public {
        requestLock.setSlashed();
        assertTrue(requestLock.isSlashed());
    }

    function testSetProverPaidAfterLockDeadlineThenSetSlashed() public {
        address prover = address(0x456);
        requestLock.setProverPaidAfterLockDeadline(prover);
        requestLock.setSlashed();

        assertTrue(requestLock.isProverPaidAfterLockDeadline());
        assertTrue(requestLock.isSlashed());
    }
}
