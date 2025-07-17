// Copyright 2025 RISC Zero, Inc.
//
// Use of this source code is governed by the Business Source License
// as found in the LICENSE-BSL file.
pragma solidity ^0.8.20;

/// @title Selector - A representation of the bytes4 selector and its index within a batch.
/// @dev This is only used as part of the AssessorJournal and AssessorReceipt.
struct Selector {
    /// @notice Index within a batch where the selector is required.
    uint16 index;
    /// @notice The actual required selector.
    bytes4 value;
}
