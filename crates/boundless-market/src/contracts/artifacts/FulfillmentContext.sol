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
    /// @notice The address to call during fulfillment. If set to address(0), no callback is performed
    address callback;
    /// @notice The gas limit to use for the callback. Must be non-zero if callback is set
    uint96 callbackGaslimit;
}

/// @title FulfillmentContextLibrary
/// @notice Library for packing, unpacking, and storing FulfillmentContext structs
/// @dev Uses bit manipulation to pack all fields into a single uint256 for transient storage
library FulfillmentContextLibrary {
    uint256 private constant VALID_MASK = 1 << 127;
    uint256 private constant PRICE_MASK = (1 << 96) - 1;
    uint256 private constant CALLBACK_MASK = (1 << 160) - 1;

    /// @notice Packs the struct into two 256-bit slots and sets the validation bit
    /// @param x The FulfillmentContext struct to pack
    /// @return slot0 First 256 bits containing valid bit and price
    /// @return slot1 Second 256 bits containing callback address and gas limit
    function pack(FulfillmentContext memory x) internal pure returns (uint256, uint256) {
        uint256 slot0 = (x.valid ? VALID_MASK : 0) | uint256(x.price);
        uint256 slot1 = uint256(uint160(x.callback)) | (uint256(x.callbackGaslimit) << 160);
        return (slot0, slot1);
    }

    /// @notice Unpacks the struct from two 128-bit slots
    /// @param slot0 First 128 bits containing valid bit and price
    /// @param slot1 Second 128 bits containing callback address and gas limit
    /// @return The unpacked FulfillmentContext struct
    /// @dev Reverts if the validation bit is not set
    function unpack(uint256 slot0, uint256 slot1) internal pure returns (FulfillmentContext memory) {
        return FulfillmentContext({
            valid: (slot0 & VALID_MASK) != 0,
            price: uint96(slot0 & PRICE_MASK),
            callback: address(uint160(slot1 & CALLBACK_MASK)),
            callbackGaslimit: uint96(slot1 >> 160)
        });
    }

    /// @notice Packs and stores the object to transient storage
    /// @param x The FulfillmentContext struct to store
    /// @param requestDigest The storage key for the transient storage
    function store(FulfillmentContext memory x, bytes32 requestDigest) internal {
        (uint256 slot0, uint256 slot1) = pack(x);
        assembly {
            tstore(requestDigest, slot0)
            tstore(add(requestDigest, 32), slot1)
        }
    }

    /// @notice Loads from transient storage and unpacks to FulfillmentContext
    /// @param requestDigest The storage key to load from
    /// @return The loaded and unpacked FulfillmentContext struct
    function load(bytes32 requestDigest) internal view returns (FulfillmentContext memory) {
        uint256 slot0;
        uint256 slot1;
        assembly {
            slot0 := tload(requestDigest)
            slot1 := tload(add(requestDigest, 32))
        }
        return unpack(slot0, slot1);
    }
}
