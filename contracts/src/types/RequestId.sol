// Copyright (c) 2025 RISC Zero, Inc.
//
// All rights reserved.
pragma solidity ^0.8.20;

import {IBoundlessMarket} from "../IBoundlessMarket.sol";

type RequestId is uint256;

using RequestIdLibrary for RequestId global;

library RequestIdLibrary {
    uint256 internal constant SMART_CONTRACT_SIGNATURE_FLAG = 1 << 192;

    /// @notice Creates a RequestId from a client address and a 32-bit index.
    /// @param client1 The address of the client.
    /// @param id The 32-bit index.
    /// @return The constructed RequestId.
    function from(address client1, uint32 id) internal pure returns (RequestId) {
        return RequestId.wrap(uint256(uint160(client1)) << 32 | uint256(id));
    }

    /// @notice Creates a RequestId from a client address, a 32-bit index, and a smart contract signature flag.
    /// @param client1 The address of the client.
    /// @param id The 32-bit index.
    /// @param isSmartContractSig Whether the request uses a smart contract signature.
    /// @return The constructed RequestId.
    function from(address client1, uint32 id, bool isSmartContractSig) internal pure returns (RequestId) {
        uint256 encoded = uint256(uint160(client1)) << 32 | uint256(id);
        if (isSmartContractSig) {
            encoded = encoded | SMART_CONTRACT_SIGNATURE_FLAG;
        }
        return RequestId.wrap(encoded);
    }

    /// @notice Extracts the client address and index from a RequestId.
    /// @param id The RequestId to extract from.
    /// @return The client address and the 32-bit index.
    function clientAndIndex(RequestId id) internal pure returns (address, uint32) {
        uint256 unwrapped = RequestId.unwrap(id);
        if (unwrapped & (type(uint256).max << 193) != 0) {
            revert IBoundlessMarket.InvalidRequest();
        }
        return (address(uint160(unwrapped >> 32)), uint32(unwrapped));
    }

    /// @notice Extracts the client address and index from a RequestId.
    /// @param id The RequestId to extract from.
    /// @return The client address and the 32-bit index, and true if the signature is a smart contract signature.
    function clientIndexAndSignatureType(RequestId id) internal pure returns (address, uint32, bool) {
        uint256 unwrapped = RequestId.unwrap(id);
        if (unwrapped & (type(uint256).max << 193) != 0) {
            revert IBoundlessMarket.InvalidRequest();
        }
        return (address(uint160(unwrapped >> 32)), uint32(unwrapped), (unwrapped & SMART_CONTRACT_SIGNATURE_FLAG) != 0);
    }

    function clientAndIsSmartContractSigned(RequestId id) internal pure returns (address, bool) {
        uint256 unwrapped = RequestId.unwrap(id);
        return (address(uint160(unwrapped >> 32)), (unwrapped & SMART_CONTRACT_SIGNATURE_FLAG) != 0);
    }

    function isSmartContractSigned(RequestId id) internal pure returns (bool) {
        uint256 unwrapped = RequestId.unwrap(id);
        return (unwrapped & SMART_CONTRACT_SIGNATURE_FLAG) != 0;
    }
}
