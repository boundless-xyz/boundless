// Copyright 2025 RISC Zero, Inc.
//
// Use of this source code is governed by the Business Source License
// as found in the LICENSE-BSL file.
pragma solidity ^0.8.20;

import {RequestId} from "./RequestId.sol";
import {PredicateType} from "./Predicate.sol";

using FulfillmentLibrary for Fulfillment global;

/// @title Fulfillment Struct and Library
/// @notice Represents the information posted by the prover to fulfill a request and get paid.
struct Fulfillment {
    /// @notice ID of the request that is being fulfilled.
    RequestId id;
    /// @notice EIP-712 digest of request struct.
    bytes32 requestDigest;
    /// @notice The `PredicateType` of the request that is being fulfilled.
    /// @dev When the `PredicateType` is `ClaimDigestMatch`, there callbacks are not supported
    PredicateType predicateType;
    /// @notice Claim Digest
    bytes32 claimDigest;
    /// @notice The callback data, if requested.
    bytes callbackData;
    /// @notice Cryptographic proof for the validity of the execution results.
    /// @dev This will be sent to the `IRiscZeroVerifier` associated with this contract.
    bytes seal;
}

library FulfillmentLibrary {}
