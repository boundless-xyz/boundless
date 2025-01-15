// Copyright (c) 2025 RISC Zero, Inc.
//
// All rights reserved.
pragma solidity ^0.8.20;

import {IBoundlessMarket} from "../IBoundlessMarket.sol";

type RequestId is uint256;

using RequestIdLibrary for RequestId global;

library RequestIdLibrary {
    /// @notice Creates a RequestId from a client address and a 32-bit index.
    /// @param client1 The address of the client.
    /// @param id The 32-bit index.
    /// @return The constructed RequestId.
    function from(address client1, uint32 id) internal pure returns (RequestId) {
        return RequestId.wrap(uint256(uint160(client1)) << 32 | uint256(id));
    }

    /// @notice Extracts the client address and index from a RequestId.
    /// @param id The RequestId to extract from.
    /// @return The client address and the 32-bit index.
    function clientAndIndex(RequestId id) internal pure returns (address, uint32) {
        uint256 unwrapped = RequestId.unwrap(id);
        if (unwrapped & (uint256(type(uint64).max) << 192) != 0) {
            revert IBoundlessMarket.InvalidRequest();
        }
        return (address(uint160(unwrapped >> 32)), uint32(unwrapped));
    }
}
