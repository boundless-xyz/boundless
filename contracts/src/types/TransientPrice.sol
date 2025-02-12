// Copyright (c) 2025 RISC Zero, Inc.
//
// All rights reserved.
pragma solidity ^0.8.24;

using TransientPriceLibrary for TransientPrice global;

/// Struct encoding the validated price for a request, intended for use with transient storage.
struct TransientPrice {
    /// Boolean set to true to indicate the request was validated.
    bool valid;
    uint96 price;
}

library TransientPriceLibrary {
    /// Packs the struct into a uint256.
    function pack(TransientPrice memory x) internal pure returns (uint256) {
        return (uint256(x.valid ? 1 : 0) << 96) | uint256(x.price);
    }

    /// Unpacks the struct from a uint256.
    function unpack(uint256 packed) internal pure returns (TransientPrice memory) {
        return TransientPrice({valid: (packed & (1 << 96)) > 0, price: uint96(packed & uint256(type(uint96).max))});
    }

    /// Packs and stores the object to transient storage.
    function store(TransientPrice memory x, bytes32 requestDigest) internal {
        uint256 packed = x.pack();
        assembly {
            tstore(requestDigest, packed)
        }
    }

    /// Loads from transient storage and unpacks to TransientPrice.
    function load(bytes32 requestDigest) internal view returns (TransientPrice memory) {
        uint256 packed;
        assembly {
            packed := tload(requestDigest)
        }
        return unpack(packed);
    }
}
