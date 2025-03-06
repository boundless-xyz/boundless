// Copyright (c) 2025 RISC Zero, Inc.
//
// All rights reserved.
pragma solidity ^0.8.24;

using FulfillmentContextLibrary for FulfillmentContext global;

/// @title FulfillmentContext
/// @notice A struct for storing validated fulfillment information in transient storage
/// @dev This struct is designed to be packed into a single uint256 for efficient transient storage
struct FulfillmentContext {
    /// @notice Boolean set to true to indicate the request was validated
    bool valid;
    /// @notice The validated price for the request
    uint96 price;
}

/// @title FulfillmentContextLibrary
/// @notice Library for packing, unpacking, and storing FulfillmentContext structs
/// @dev Uses bit manipulation to pack all fields into a single uint256 for transient storage
library FulfillmentContextLibrary {
    uint256 private constant VALID_MASK = 1 << 127;
    uint256 private constant PRICE_MASK = (1 << 96) - 1;

    /// @notice Packs the struct into a single 256-bit slots and sets the validation bit.
    /// @param x The FulfillmentContext struct to pack
    /// @return Packed uint256 containing valid bit and price
    function pack(FulfillmentContext memory x) internal pure returns (uint256) {
        return (x.valid ? VALID_MASK : 0) | uint256(x.price);
    }

    /// @notice Unpacks the struct from a single uint256
    /// @param packed Packed uint256 containing the valid bit and price
    /// @return The unpacked FulfillmentContext struct
    function unpack(uint256 packed) internal pure returns (FulfillmentContext memory) {
        return FulfillmentContext({valid: (packed & VALID_MASK) != 0, price: uint96(packed & PRICE_MASK)});
    }

    /// @notice Packs and stores the object to transient storage
    /// @param x The FulfillmentContext struct to store
    /// @param requestDigest The storage key for the transient storage
    function store(FulfillmentContext memory x, bytes32 requestDigest) internal {
        uint256 packed = pack(x);
        assembly {
            tstore(requestDigest, packed)
        }
    }

    /// @notice Loads from transient storage and unpacks to FulfillmentContext
    /// @param requestDigest The storage key to load from
    /// @return The loaded and unpacked FulfillmentContext struct
    function load(bytes32 requestDigest) internal view returns (FulfillmentContext memory) {
        uint256 packed;
        assembly {
            packed := tload(requestDigest)
        }
        return unpack(packed);
    }
}
