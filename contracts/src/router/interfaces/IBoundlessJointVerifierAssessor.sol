// Copyright 2026 Boundless Foundation, Inc.
//
// Use of this source code is governed by the Business Source License
// as found in the LICENSE-BSL file.
// SPDX-License-Identifier: BUSL-1.1

pragma solidity ^0.8.26;

import {SlimRequest} from "../../types/SlimRequest.sol";
import {Fulfillment} from "../../types/Fulfillment.sol";

/// @title IBoundlessJointVerifierAssessor — per-fill combined verifier + binding.
///
/// @notice An adapter implementing this interface vouches in one call for the
///         cryptographic check on `fill.seal`, the binding between `requestDigest`
///         and `fill.claimDigest`, AND the binding to `prover` (the address the
///         market will credit / slash). Used by classes whose underlying mechanism
///         is naturally per-fill, where splitting the check across two seams would
///         waste a dispatch and force batched signing.
///
/// @dev    The adapter receives the full per-fill payload — the slim request, the
///         fulfillment, the domain-bound `requestDigest` (already reconstructed and
///         binding-checked by the market), and the `prover` the market will credit.
///         It can use whatever subset its mechanism needs; the router does not
///         narrow the surface to specific fields so future joint adapters (e.g.
///         attestation-based, predicate-aware) can grow without an interface change.
///
///         `prover` is forwarded by the router as a universal arg, identical across
///         every fill in the batch. Each adapter is responsible for binding it via
///         its own mechanism — a signature-based adapter would include `prover` in
///         the signing payload alongside `(requestDigest, claimDigest)`. The market
///         trusts the adapter to have verified the binding.
///
///         Classes whose `interfaceTag == type(IBoundlessJointVerifierAssessor).interfaceId`
///         do NOT carry an assessor seam — `requiredAssessorClass` must be 0x00 and the
///         router skips the per-batch assessor call for batches under such classes.
interface IBoundlessJointVerifierAssessor {
    /// @notice Verify, for one fill, that `fill.seal` cryptographically attests
    ///         `fill.claimDigest`, that `fill.claimDigest` is the correct binding
    ///         for `requestDigest`, and that `fill.seal` also commits to `prover`.
    ///         Reverts on failure.
    /// @param request       Slim payload for this fill (pre-bound by the market).
    ///                      Carries the requestor's selector, callback, predicate,
    ///                      and the pre-computed input/offer/imageUrl digests.
    /// @param fill          The fulfillment whose `claimDigest` and `seal` the
    ///                      adapter verifies. `fulfillmentData` is available for
    ///                      adapters that need to evaluate the predicate or
    ///                      reconstruct a journal.
    /// @param requestDigest Domain-bound EIP-712 hash of the original
    ///                      `ProofRequest`. Already reconstructed and
    ///                      binding-checked by the market against the lock /
    ///                      `FulfillmentContext`.
    /// @param prover        Address the market credits / slashes.
    function verifyJoint(SlimRequest calldata request, Fulfillment calldata fill, bytes32 requestDigest, address prover)
        external
        view;
}
