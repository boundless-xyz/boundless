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
            lockDeadline: uint64(block.timestamp + 100),
            deadlineDelta: uint24(50),
            requestLockFlags: 0,
            price: 1 ether,
            stake: 1 ether,
            fingerprint: bytes8(0x1234567890abcdef)
        });
    }

    function assertSlotClear(uint256 slotNumber) private view {
        uint256 slot;
        assembly {
            let num := add(requestLock.slot, slotNumber)
            slot := sload(num)
        }
        assertEq(slot, 0, "Slot is not zero");
    }

    function assertSlot1Clear() private view {
        assertSlotClear(1);
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
        assertSlot1Clear();
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
        assertSlot1Clear();
    }

    function testIsProverPaidBeforeLockDeadline() public {
        requestLock.setProverPaidBeforeLockDeadline();
        assertTrue(requestLock.isProverPaidBeforeLockDeadline());
    }

    function testIsProverPaidAfterLockDeadline() public {
        requestLock.setProverPaidAfterLockDeadline(address(0x456));
        assertTrue(requestLock.isProverPaidAfterLockDeadline());
        assertNotEq(requestLock.price, 0, "Price not zeroed out");
        assertNotEq(requestLock.stake, 0, "Stake not zeroed out");
        assertNotEq(requestLock.fingerprint, bytes8(0), "Fingerprint not zeroed out");
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
