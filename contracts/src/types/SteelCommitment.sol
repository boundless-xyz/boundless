// Copyright (c) 2025 RISC Zero, Inc.
//
// All rights reserved.
pragma solidity ^0.8.20;

/// @title SteelCommitment Struct
/// @notice Represents a commitment to a specific block in the blockchain.
/// @dev The `id` combines the version and the actual identifier of the claim, such as the block number.
/// @dev The `digest` represents the data being committed to, e.g. the hash of the execution block.
/// @dev The `configID` is the cryptographic digest of the network configuration
struct SteelCommitment {
    /// @notice The ID of the commitment.
    uint256 id;
    /// @notice The digest of the commitment.
    bytes32 digest;
    /// @notice The configuration ID of the commitment.
    bytes32 configID;
}
