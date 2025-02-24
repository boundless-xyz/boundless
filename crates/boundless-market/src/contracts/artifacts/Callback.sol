// Copyright (c) 2025 RISC Zero, Inc.
//
// All rights reserved.
pragma solidity ^0.8.24;

using CallbackLibrary for Callback global;

/// @title Callback Struct and Library
/// @notice Represents a callback configuration for proof delivery
struct Callback {
    /// @notice The address of the contract to call back
    address addr;
    /// @notice Maximum gas to use for the callback
    uint96 gasLimit;
}

library CallbackLibrary {
    string constant CALLBACK_TYPE = "Callback(address addr,uint96 gasLimit)";
    bytes32 constant CALLBACK_TYPEHASH = keccak256(bytes(CALLBACK_TYPE));

    /// @notice Computes the EIP-712 digest for the given callback
    /// @param callback The callback to compute the digest for
    /// @return The EIP-712 digest of the callback
    function eip712Digest(Callback memory callback) internal pure returns (bytes32) {
        return keccak256(abi.encode(CALLBACK_TYPEHASH, callback.addr, callback.gasLimit));
    }
}
