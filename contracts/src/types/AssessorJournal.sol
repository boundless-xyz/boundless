// Copyright (c) 2025 RISC Zero, Inc.
//
// All rights reserved.
pragma solidity ^0.8.20;

import {SteelCommitment} from "./SteelCommitment.sol";
import {AssessorCallback} from "./AssessorCallback.sol";
import {Selector} from "./Selector.sol";

/// @title Assessor Journal Struct
/// @notice Represents the structured journal of the Assessor guest which verifies the signature(s)
/// from client(s) and that the requirements are met by claim digest(s) in the Merkle tree committed
/// to by the given root.
struct AssessorJournal {
    /// @notice Digest of each request validated by the assessor.
    /// @dev When a client signs two requests with the same ID, only one can ever be fulfilled.
    /// Using the digest here ensures that the request validated by the assessor matches the one that was locked / priced.
    bytes32[] requestDigests;
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
    /// @notice Steel block commitment against which any ERC-1271 smart contract signatures are verified.
    /// @dev If no requests in the batch are authorized by smart contract signatures, the prover is
    /// not required to provide an EVM env. In this case, this field will be set to all zeroes.
    SteelCommitment steel_commitment;
}
