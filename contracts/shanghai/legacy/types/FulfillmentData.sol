// Copyright 2026 Boundless Foundation, Inc.
//
// Use of this source code is governed by the Business Source License
// as found in the LICENSE-BSL file.
pragma solidity ^0.8.26;

using FulfillmentDataLibrary for FulfillmentDataImageIdAndJournal global;

enum FulfillmentDataType {
    None,
    ImageIdAndJournal
}

/// @title FulfillmentDataImageIdAndJournal Struct and Library
/// @notice Represents a fulfillment where the image id and journal are delivered
struct FulfillmentDataImageIdAndJournal {
    /// @notice Image ID of the guest that was verifiably executed to satisfy the request.
    bytes32 imageId;
    /// @notice Journal committed by the guest program execution.
    bytes journal;
}

library FulfillmentDataLibrary {
    /// @notice Decodes a bytes calldata into a FulfillmentDataImageIdAndJournal struct.
    /// @param data The bytes calldata to decode.
    /// @return fillData The decoded FulfillmentDataImageIdAndJournal struct.
    function decodeFulfillmentDataImageIdAndJournal(bytes calldata data)
        public
        pure
        returns (FulfillmentDataImageIdAndJournal memory fillData)
    {
        (fillData.imageId, fillData.journal) = decodePackedImageIdAndJournal(data);
    }

    /// @notice Decodes a bytes calldata into a the image id and journal.
    /// @param data The bytes calldata to decode.
    /// @return imageId The decoded image ID.
    /// @return journal The decoded journal.
    function decodePackedImageIdAndJournal(bytes calldata data)
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
}
