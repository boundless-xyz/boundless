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
    /// @notice The commitment of the journal.
    SteelCommitment commitment;
    /// @notice Cryptographic proof for the validity of the execution results.
    /// @dev This will be sent to the `IRiscZeroVerifier` associated with this contract.
    bytes seal;
    /// @notice Optional callbacks committed into the journal.
    AssessorCallback[] callbacks;
    /// @notice Optional selectors committed into the journal.
    Selector[] selectors;
    /// @notice Address of the prover
    address prover;
}
