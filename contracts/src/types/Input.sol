// Copyright 2025 RISC Zero, Inc.
//
// Use of this source code is governed by the Business Source License
// as found in the LICENSE-BSL file.
pragma solidity ^0.8.20;

using InputLibrary for Input global;

/// @title Input Types and Library
/// @notice Provides functions to create and handle different types of inputs.
enum InputType {
    Inline,
    Url
}

/// @notice Represents an input with a type and data.
struct Input {
    InputType inputType;
    bytes data;
}

library InputLibrary {
    string constant INPUT_TYPE = "Input(uint8 inputType,bytes data)";
    bytes32 constant INPUT_TYPEHASH = keccak256(bytes(INPUT_TYPE));

    /// @notice Creates an inline input.
    /// @param inlineData The data for the inline input.
    /// @return An Input struct with type Inline and the provided data.
    function createInlineInput(bytes memory inlineData) internal pure returns (Input memory) {
        return Input({inputType: InputType.Inline, data: inlineData});
    }

    /// @notice Creates a URL input.
    /// @param url The URL for the input.
    /// @return An Input struct with type Url and the provided URL as data.
    function createUrlInput(string memory url) internal pure returns (Input memory) {
        return Input({inputType: InputType.Url, data: bytes(url)});
    }

    /// @notice Computes the EIP-712 digest for the given input.
    /// @param input The input to compute the digest for.
    /// @return The EIP-712 digest of the input.
    function eip712Digest(Input memory input) internal pure returns (bytes32) {
        return keccak256(abi.encode(INPUT_TYPEHASH, input.inputType, keccak256(input.data)));
    }
}
