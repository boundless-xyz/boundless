// Copyright (c) 2025 RISC Zero, Inc.
//
// All rights reserved.

pragma solidity ^0.8.20;

import "forge-std/Test.sol";
import {FulfillmentContext, FulfillmentContextLibrary} from "../../src/types/FulfillmentContext.sol";

contract FulfillmentContextLibraryTest is Test {
    /// forge-config: default.fuzz.runs = 10000
    function testFuzz_PackUnpack(bool valid, uint96 price, address callback, uint96 callbackGaslimit) public pure {
        FulfillmentContext memory original =
            FulfillmentContext({valid: valid, price: price, callback: callback, callbackGaslimit: callbackGaslimit});

        (uint256 slot0, uint256 slot1) = FulfillmentContextLibrary.pack(original);
        FulfillmentContext memory unpacked = FulfillmentContextLibrary.unpack(slot0, slot1);

        assertEq(unpacked.valid, original.valid, "Valid flag mismatch");
        assertEq(unpacked.price, original.price, "Price mismatch");
        assertEq(unpacked.callback, original.callback, "Callback address mismatch");
        assertEq(unpacked.callbackGaslimit, original.callbackGaslimit, "Callback gas limit mismatch");
    }

    /// forge-config: default.fuzz.runs = 10000
    function testFuzz_StoreAndLoad(bool valid, uint96 price, address callback, uint96 callbackGaslimit) public {
        FulfillmentContext memory original =
            FulfillmentContext({valid: valid, price: price, callback: callback, callbackGaslimit: callbackGaslimit});
        bytes32 slot = keccak256("transient.fulfillment.slot");

        // Store the FulfillmentContext in the specified slot
        FulfillmentContextLibrary.store(original, slot);

        // Load the FulfillmentContext from the specified slot
        FulfillmentContext memory loaded = FulfillmentContextLibrary.load(slot);

        // Verify that the loaded FulfillmentContext matches the original
        assertEq(loaded.valid, original.valid, "Valid flag mismatch");
        assertEq(loaded.price, original.price, "Price mismatch");
        assertEq(loaded.callback, original.callback, "Callback address mismatch");
        assertEq(loaded.callbackGaslimit, original.callbackGaslimit, "Callback gas limit mismatch");
    }
}
