// Copyright 2026 Boundless Foundation, Inc.
//
// Use of this source code is governed by the Business Source License
// as found in the LICENSE-BSL file.
pragma solidity ^0.8.26;

import {RequestId} from "./RequestId.sol";
import {FulfillmentDataType} from "./FulfillmentData.sol";

using FulfillmentLibrary for Fulfillment global;

/// @title Fulfillment Struct and Library
/// @notice Represents the information posted by the prover to fulfill a request and get paid.
struct Fulfillment {
    /// @notice ID of the request that is being fulfilled.
    RequestId id;
    /// @notice EIP-712 digest of request struct.
    bytes32 requestDigest;
    /// @notice Claim Digest
    bytes32 claimDigest;
    /// @notice The type of data included in the fulfillment
    FulfillmentDataType fulfillmentDataType;
    /// @notice The fulfillment data
    bytes fulfillmentData;
    /// @notice Cryptographic proof for the validity of the execution results.
    /// @dev This will be sent to the `IRiscZeroVerifier` associated with this contract.
    bytes seal;
}

library FulfillmentLibrary {
    /// @notice Computes the digest of the fulfillment data that is committed to by the assessor.
    /// @param fulfillment The Fulfillment struct containing potentially the journal
    /// @return The keccak256 digest of the fulfillmentData.
    function fulfillmentDataDigest(Fulfillment memory fulfillment) internal pure returns (bytes32) {
        return keccak256(abi.encodePacked(uint8(fulfillment.fulfillmentDataType), fulfillment.fulfillmentData));
    }
}
