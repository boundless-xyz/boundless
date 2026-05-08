// Copyright 2026 Boundless Foundation, Inc.
//
// Use of this source code is governed by the Business Source License
// as found in the LICENSE-BSL file.
// SPDX-License-Identifier: BUSL-1.1

pragma solidity ^0.8.26;

import {Fulfillment} from "./Fulfillment.sol";
import {ProofRequest} from "./ProofRequest.sol";

/// @title SubBatch — single-class slice of a fulfillment transaction.
///
/// @notice A `SubBatch` carries the data the market and router need to verify and
///         settle one verifier-class group of fills. One transaction can carry
///         multiple sub-batches of mixed classes; each is verified independently
///         by the router and settles its own per-fill lifecycle.
///
///         All fills in a sub-batch must share the same verifier class (the router
///         enforces this via `MixedClassWithinSubBatch`). The optional assessor
///         seam is per-sub-batch: verifier-class sub-batches carry a non-empty
///         `assessorSeal`, joint-class sub-batches must leave it empty.
///
///         The market re-derives each request's EIP-712 digest at fulfill time
///         (asserts against the lock for locked requests, against the signature
///         for unlocked requests in `priceAndFulfill`). `signedSelectors` and
///         per-fill `callback` config are read directly from the verified
///         `requests`, not from any assessor journal.
struct SubBatch {
    /// @notice Per-fill `ProofRequest` (one per `fills` entry, same order).
    ///         The market re-derives `requestDigest = requests[i].eip712Digest()`
    ///         and asserts integrity against the lock or signature.
    ProofRequest[] requests;
    /// @notice Per-fill `Fulfillment` (one per `requests` entry, same order).
    Fulfillment[] fills;
    /// @notice Bytes for the assessor call. First 4 bytes are the BoundlessRouter
    ///         assessor selector; the rest is the per-class envelope. Must be
    ///         empty for joint-class sub-batches.
    bytes assessorSeal;
    /// @notice Address the market will credit / slash for this sub-batch. The
    ///         router forwards this to the assessor (or joint) adapter, which
    ///         binds it via its own mechanism. The market trusts the resulting
    ///         attested value.
    address prover;
}
