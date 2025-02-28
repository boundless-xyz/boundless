// Copyright (c) 2025 RISC Zero, Inc.
//
// All rights reserved.
pragma solidity ^0.8.20;

/// @title Selector - A representation of the bytes4 selector and its index within a batch.
/// @dev This is only used as part of the AssessorJournal and AssessorReceipt.
struct Selector {
    /// @notice Index within a bach where the selector is required.
    uint16 index;
    /// @notice The actual required selector.
    bytes4 value;
}
