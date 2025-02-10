// Copyright (c) 2025 RISC Zero, Inc.
//
// All rights reserved.
pragma solidity ^0.8.20;

using SelectorLib for Selector global;
using SelectorsLib for Selectors global;

/// @title Selector - A type for handling optional bytes4 values
/// @notice Provides a safe way to handle optional values in Solidity
struct Selector {
    /// @notice The wrapped value, only valid when isSome is true
    bytes4 value;
    /// @notice Indicates whether the Option contains a value (true) or is None (false)
    bool isSome;
}

/// @title Selectors - A sparse representation of bytes4 selectors
/// @notice Efficiently stores selectors when most are zero
struct Selectors {
    /// @notice Indices where selectors are required
    uint8[] indices;
    /// @notice The actual required selectors, corresponding to indices
    bytes4[] values;
}

/// @title SelectorLib - Helper library for working with Option types
/// @notice Provides utility functions for creating and manipulating Option values
library SelectorLib {
    /// @notice The EIP-712 typehash for Selector
    bytes32 public constant SELECTOR_TYPEHASH = keccak256("Selector(bytes4 value,bool isSome)");

    /// @notice Creates an Selector containing a value (Some variant)
    /// @param value The value to wrap in an Selector
    /// @return An Selector containing the provided value
    function some(bytes4 value) internal pure returns (Selector memory) {
        return Selector({value: value, isSome: true});
    }

    /// @notice Creates an empty Selector (None variant)
    /// @return An Selector representing None
    function none() internal pure returns (Selector memory) {
        return Selector({value: bytes4(0), isSome: false});
    }

    /// @notice Safely extracts the value from an Selector
    /// @param opt The Selector to unwrap
    /// @return The contained value
    /// @custom:throws "Selector: unwrap called on None" if the Selector is None
    function unwrap(Selector memory opt) internal pure returns (bytes4) {
        require(opt.isSome, "Selector: unwrap called on None");
        return opt.value;
    }

    /// @notice Extracts the value from an Selector with a fallback
    /// @param opt The Selector to unwrap
    /// @param defaultValue The value to return if the Selector is None
    /// @return The contained value or the default
    function unwrapOr(Selector memory opt, bytes4 defaultValue) internal pure returns (bytes4) {
        return opt.isSome ? opt.value : defaultValue;
    }

    /// @notice Computes the EIP-712 digest for the given Selector
    /// @param opt The Selector to compute the digest for
    /// @return The EIP-712 digest of the Selector
    function eip712Digest(Selector memory opt) internal pure returns (bytes32) {
        return keccak256(abi.encode(SELECTOR_TYPEHASH, opt.value, opt.isSome));
    }
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
