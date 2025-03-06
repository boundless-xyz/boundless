// Copyright (c) 2025 RISC Zero, Inc.
//
// All rights reserved.
pragma solidity ^0.8.20;

import {AssessorCallback} from "./AssessorCallback.sol";
import {Selector} from "./Selector.sol";
import {SteelCommitment} from "./SteelCommitment.sol";

/// @title AssessorReceipt Struct and Library
/// @notice Represents the output of the assessor and proof of correctness, allowing request fulfillment.
struct AssessorReceipt {
    /// @notice Cryptographic proof for the validity of the execution results.
    /// @dev This will be sent to the `IRiscZeroVerifier` associated with this contract.
    bytes seal;
    /// @notice Optional callbacks committed into the journal.
    AssessorCallback[] callbacks;
    /// @notice Optional selectors committed into the journal.
    Selector[] selectors;
    /// @notice Address of the prover
    address prover;
    /// @notice Steel block commitment against which any ERC-1271 smart contract signatures are verified.
    /// @dev If no requests in the batch are authorized by smart contract signatures, the prover is
    /// not required to provide an EVM env. In this case, this field will be set to all zeroes.
    SteelCommitment steel_commitment;
}
