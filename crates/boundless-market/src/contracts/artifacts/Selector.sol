// Copyright (c) 2025 RISC Zero, Inc.
//
// All rights reserved.
pragma solidity ^0.8.20;

/// @title Selectors - A sparse representation of bytes4 selectors
/// @notice Efficiently stores selectors when most are zero
struct Selectors {
    /// @notice Indices where selectors are required
    uint8[] indices;
    /// @notice The actual required selectors, corresponding to indices
    bytes4[] values;
}
