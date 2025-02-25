// Copyright (c) 2025 RISC Zero, Inc.
//
// All rights reserved.
pragma solidity ^0.8.20;

import {Selector} from "./Selector.sol";
import {RequestId} from "./RequestId.sol";

using FulfillmentAssessorLibrary for FulfillmentAssessor global;

/// @title FulfillmentAssessor Struct and Library
/// @notice Represents the information posted by the prover that fulfills an Assessor.
struct FulfillmentAssessor {
    /// @notice Cryptographic proof for the validity of the execution results.
    /// @dev This will be sent to the `IRiscZeroVerifier` associated with this contract.
    bytes seal;
    /// @notice Optional selectors committed into the journal.
    Selector[] selectors;
    /// @notice Address of the prover
    address prover;
}

library FulfillmentAssessorLibrary {}
