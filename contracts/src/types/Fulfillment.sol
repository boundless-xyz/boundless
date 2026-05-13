// Copyright 2026 Boundless Foundation, Inc.
//
// Use of this source code is governed by the Business Source License
// as found in the LICENSE-BSL file.
pragma solidity ^0.8.26;

import {FulfillmentDataType} from "./FulfillmentData.sol";

using FulfillmentLibrary for Fulfillment global;

/// @title Fulfillment Struct and Library
/// @notice The proof material the prover posts to fulfill a request. The request
///         identity (`id`, `requestDigest`) is carried by the paired
///         `SlimRequest` in `SubBatch.requests` — the market re-binds them
///         positionally and trusts the slim payload after the binding check
///         in `_verifyBinding`.
struct Fulfillment {
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

    /// @notice Calldata-friendly variant of `fulfillmentDataDigest`. Takes the
    ///         primitive fields directly so callers holding a `Fulfillment
    ///         calldata` reference can avoid copying the full struct
    ///         (including `seal` bytes) to memory just to hash the data
    ///         portion. Produces a result byte-identical to the memory form.
    function fulfillmentDataDigest(FulfillmentDataType dtype, bytes calldata data) internal pure returns (bytes32) {
        return keccak256(abi.encodePacked(uint8(dtype), data));
    }
}
