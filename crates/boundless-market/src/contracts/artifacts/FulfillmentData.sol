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
    /// @notice Decodes a bytes calldata into a FulfillmentData struct.
    /// @param data The bytes calldata to decode.
    /// @return fillData The decoded FulfillmentData struct.
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

    /// @notice Decodes the fulfillment data from a bytes calldata.
    /// @param data The bytes calldata to decode.
    /// @return imageId The decoded image ID.
    /// @return journal The decoded journal.
    function decodeFulfillmentData(bytes calldata data)
        internal
        pure
        returns (bytes32 imageId, bytes calldata journal)
    {
        assembly {
            // Extract imageId (first 32 bytes after length)
            imageId := calldataload(add(data.offset, 0x20))
            // Extract journal offset and create calldata slice
            let journalOffset := calldataload(add(data.offset, 0x40))
            let journalPtr := add(data.offset, add(0x20, journalOffset))
            let journalLength := calldataload(journalPtr)
            journal.offset := add(journalPtr, 0x20)
            journal.length := journalLength
        }
    }

    /// @notice Encodes the fulfillment data into bytes.
    /// @param fillData The FulfillmentData struct to encode.
    /// @return The encoded bytes.
    function encode(FulfillmentData memory fillData) public pure returns (bytes memory) {
        return abi.encode(fillData);
    }
}
