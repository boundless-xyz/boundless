// Copyright (c) 2025 RISC Zero, Inc.
//
// All rights reserved.

pragma solidity ^0.8.20;

import "forge-std/Test.sol";
import {TransientPrice, TransientPriceLibrary} from "../../src/types/TransientPrice.sol";

contract TransientPriceLibraryTest is Test {
    /// forge-config: default.fuzz.runs = 10000
    function testFuzz_PackUnpack(bool valid, uint96 price) public pure {
        TransientPrice memory original = TransientPrice({valid: valid, price: price});

        uint256 packed = TransientPriceLibrary.pack(original);
        TransientPrice memory unpacked = TransientPriceLibrary.unpack(packed);

        assertEq(unpacked.valid, original.valid, "Valid flag mismatch");
        assertEq(unpacked.price, original.price, "Price mismatch");
    }

    /// forge-config: default.fuzz.runs = 10000
    function testFuzz_StoreAndLoad(bool valid, uint96 price) public {
        TransientPrice memory original = TransientPrice({valid: valid, price: price});
        bytes32 slot = keccak256("transient.price.slot");

        // Store the TransientPrice in the specified slot
        TransientPriceLibrary.store(original, slot);

        // Load the TransientPrice from the specified slot
        TransientPrice memory loaded = TransientPriceLibrary.load(slot);

        // Verify that the loaded TransientPrice matches the original
        assertEq(loaded.valid, original.valid, "Valid flag mismatch");
        assertEq(loaded.price, original.price, "Price mismatch");
    }
}
