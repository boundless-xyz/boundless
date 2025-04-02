// Copyright (c) 2025 RISC Zero, Inc.
//
// All rights reserved.
pragma solidity ^0.8.24;

/// @title IBoundlessMarketCallback
/// @notice Interface for handling proof callbacks from BoundlessMarket with proof verification
/// @dev Inherit from this contract to implement custom proof handling logic for BoundlessMarket proofs
interface IBoundlessMarketCallback {
    /// @notice Handles submitting proofs with RISC Zero proof verification
    /// @param imageId The ID of the RISC Zero guest image that produced the proof
    /// @param journal The output journal from the RISC Zero guest execution
    /// @param seal The cryptographic seal proving correct execution
    function handleProof(bytes32 imageId, bytes calldata journal, bytes calldata seal) external;
}
