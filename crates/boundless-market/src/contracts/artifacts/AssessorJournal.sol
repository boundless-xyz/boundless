// Copyright (c) 2025 RISC Zero, Inc.
//
// All rights reserved.
pragma solidity ^0.8.20;

import {AssessorCallback} from "./AssessorCallback.sol";
import {Selector} from "./Selector.sol";

/// @title Assessor Journal Struct
/// @notice Represents the structured journal of the Assessor guest which verifies the signature(s)
/// from client(s) and that the requirements are met by claim digest(s) in the Merkle tree committed
/// to by the given root.
struct AssessorJournal {
    /// @notice The (optional) callbacks for the requests committed by the assessor.
    AssessorCallback[] callbacks;
    /// @notice The (optional) selectors for the requests committed by the assessor.
    /// @dev This is used to verify the fulfillment of the request against its selector's seal.
    Selector[] selectors;
    /// @notice Root of the Merkle tree committing to the set of proven claims.
    /// @dev In the case of a batch of size one, this may simply be a claim digest.
    bytes32 root;
    /// @notice The address of the prover that produced the assessor receipt.
    address prover;
}
