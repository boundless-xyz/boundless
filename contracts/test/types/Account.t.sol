// Copyright (c) 2025 RISC Zero, Inc.
//
// All rights reserved.

pragma solidity ^0.8.20;

import "forge-std/Test.sol";
import {Account, AccountLibrary} from "../../src/types/Account.sol";

/// @dev Wrapper contract to test Account. Declaring Account as a storage variable
/// directly in the test contract makes it type `StdCheatsSafe.Account` which causes
/// the library functions on type `Account` to not be available.
contract AccountTestContract {
    Account account;

    function requestFlags(uint32 idx) public view returns (bool, bool) {
        return account.requestFlags(idx);
    }

    function setRequestLocked(uint32 idx) public {
        account.setRequestLocked(idx);
    }

    function setRequestFulfilled(uint32 idx) public {
        account.setRequestFulfilled(idx);
    }
}

contract AccountTest is Test {
    AccountTestContract account = new AccountTestContract();

    function testRequestFlags() public {
        uint32 idx = 5;

        // Initially, the request should not be locked or fulfilled
        (bool locked, bool fulfilled) = account.requestFlags(idx);
        assertFalse(locked, "Request should not be locked initially");
        assertFalse(fulfilled, "Request should not be fulfilled initially");

        // Set the request as locked
        account.setRequestLocked(idx);
        (locked, fulfilled) = account.requestFlags(idx);
        assertTrue(locked, "Request should be locked");
        assertFalse(fulfilled, "Request should not be fulfilled");

        // Set the request as fulfilled
        account.setRequestFulfilled(idx);
        (locked, fulfilled) = account.requestFlags(idx);
        assertTrue(locked, "Request should be locked");
        assertTrue(fulfilled, "Request should be fulfilled");
    }
}
