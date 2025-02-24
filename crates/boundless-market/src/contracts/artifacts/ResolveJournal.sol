// Copyright (c) 2025 RISC Zero, Inc.
//
// All rights reserved.
pragma solidity ^0.8.20;

/// @title Resolve Journal Struct
/// @notice Represents the structured journal of the Resolve guest which resolves composed proofs.
struct ResolveJournal {
    /// @notice Image ID of the SetBuilder guest.
    bytes32 setBuilderImageID;
    /// @notice Digest of input conditional claim after its first assumption has been resolved.
    bytes32 claimDigest;
}
