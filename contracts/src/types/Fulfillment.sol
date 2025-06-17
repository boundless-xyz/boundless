// Copyright 2025 RISC Zero, Inc.
//
// Use of this source code is governed by the Business Source License
// as found in the LICENSE-BSL file.
pragma solidity ^0.8.20;

import {RequestId} from "./RequestId.sol";

using FulfillmentLibrary for Fulfillment global;

/// @title Fulfillment Struct and Library
/// @notice Represents the information posted by the prover to fulfill a request and get paid.
struct Fulfillment {
    /// @notice ID of the request that is being fulfilled.
    RequestId id;
    /// @notice EIP-712 digest of request struct.
    bytes32 requestDigest;
    /// @notice Image ID of the guest that was verifiably executed to satisfy the request.
    /// @dev Must match the value in the request's requirements.
    bytes32 imageId;
    // TODO: Add a flag in the request to decide whether to post the journal. Note that
    // if the journal and journal digest do not need to be delivered to the client, imageId will
    // be replaced with claim digest, since it is captured in the requirements on the request,
    // checked by the Assessor guest.
    /// @notice Journal committed by the guest program execution.
    /// @dev The journal is checked to satisfy the predicate specified on the request's requirements.
    bytes journal;
    /// @notice Cryptographic proof for the validity of the execution results.
    /// @dev This will be sent to the `IRiscZeroVerifier` associated with this contract.
    bytes seal;
}

library FulfillmentLibrary {}
