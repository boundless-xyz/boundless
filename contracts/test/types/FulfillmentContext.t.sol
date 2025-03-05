// Copyright (c) 2025 RISC Zero, Inc.
//
// All rights reserved.

pragma solidity ^0.8.20;

import "forge-std/Test.sol";
import {FulfillmentContext, FulfillmentContextLibrary} from "../../src/types/FulfillmentContext.sol";

contract FulfillmentContextLibraryTest is Test {
    /// forge-config: default.fuzz.runs = 10000
    function testFuzz_PackUnpack(bool valid, uint96 price) public pure {
        FulfillmentContext memory original = FulfillmentContext({valid: valid, price: price});

        uint256 packed = FulfillmentContextLibrary.pack(original);
        FulfillmentContext memory unpacked = FulfillmentContextLibrary.unpack(packed);

        assertEq(unpacked.valid, original.valid, "Valid flag mismatch");
        assertEq(unpacked.price, original.price, "Price mismatch");
    }

    /// forge-config: default.fuzz.runs = 10000
    function testFuzz_StoreAndLoad(bool valid, uint96 price) public {
        FulfillmentContext memory original = FulfillmentContext({valid: valid, price: price});
        bytes32 slot = keccak256("transient.fulfillment.slot");

        // Store the FulfillmentContext in the specified slot
        FulfillmentContextLibrary.store(original, slot);

        // Load the FulfillmentContext from the specified slot
        FulfillmentContext memory loaded = FulfillmentContextLibrary.load(slot);

        // Verify that the loaded FulfillmentContext matches the original
        assertEq(loaded.valid, original.valid, "Valid flag mismatch");
        assertEq(loaded.price, original.price, "Price mismatch");
    }
}
