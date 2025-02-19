// Copyright (c) 2025 RISC Zero, Inc.
//
// All rights reserved.
pragma solidity ^0.8.20;

using SelectorsLib for Selectors global;

/// @title Selectors - A sparse representation of bytes4 selectors
/// @notice Efficiently stores selectors when most are zero
struct Selectors {
    /// @notice Indices where selectors are required
    uint8[] indices;
    /// @notice The actual required selectors, corresponding to indices
    bytes4[] values;
}

/// @title SelectorsLib - Helper library for working with Selectors
/// @notice Provides utility functions for creating and manipulating Selectors
library SelectorsLib {
    /// @notice Adds a non-zero selector at the given index
    /// @dev Overwrites any existing selector at that index
    /// @param self The Selectors struct to modify
    /// @param index The index where to add the selector
    /// @param selector The selector to add
    function add(Selectors memory self, uint8 index, bytes4 selector) internal pure {
        // Check if index already exists
        for (uint256 i = 0; i < self.indices.length; i++) {
            if (self.indices[i] == index) {
                // Update existing
                self.values[i] = selector;
                return;
            }
        }

        // Create new arrays with increased size
        uint8[] memory newIndices = new uint8[](self.indices.length + 1);
        bytes4[] memory newSelectors = new bytes4[](self.values.length + 1);

        // Copy existing elements
        for (uint256 i = 0; i < self.indices.length; i++) {
            newIndices[i] = self.indices[i];
            newSelectors[i] = self.values[i];
        }

        // Add new element
        newIndices[self.indices.length] = index;
        newSelectors[self.values.length] = selector;

        // Update arrays
        self.indices = newIndices;
        self.values = newSelectors;
    }
}
