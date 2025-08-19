// Copyright 2025 RISC Zero, Inc.
//
// Use of this source code is governed by the Business Source License
// as found in the LICENSE-BSL file.
pragma solidity ^0.8.24;

using FulfillmentDataLibrary for FulfillmentData global;

enum FulfillmentDataType {
    None,
    ImageIdAndJournal
}

/// @title FulfillmentData Struct and Library
/// @notice Represents a fulfillment configuration for proof delivery
struct FulfillmentData {
    /// @notice Image ID of the guest that was verifiably executed to satisfy the request.
    bytes32 imageId;
    /// @notice Journal committed by the guest program execution.
    /// @dev The journal is checked to satisfy the predicate specified on the request's requirements.
    bytes journal;
}

library FulfillmentDataLibrary {
    function decode(bytes calldata data) public pure returns (FulfillmentData memory fillData) {
        bytes32 imageId;
        bytes calldata journal;
        assembly {
            imageId := calldataload(add(data.offset, 0x20))
            let journalOffset := calldataload(add(data.offset, 0x40))
            let journalPtr := add(data.offset, add(0x20, journalOffset))
            let journalLength := calldataload(journalPtr)
            journal.offset := add(journalPtr, 0x20)
            journal.length := journalLength
        }
        fillData = FulfillmentData(imageId, journal);
    }

    function encode(FulfillmentData memory fillData) public pure returns (bytes memory) {
        return abi.encode(fillData);
    }
}
