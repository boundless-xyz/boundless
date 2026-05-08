// Copyright 2026 Boundless Foundation, Inc.
//
// Use of this source code is governed by the Business Source License
// as found in the LICENSE-BSL file.
// SPDX-License-Identifier: BUSL-1.1

pragma solidity ^0.8.26;

/// @title IBoundlessAssessor — per-batch binding seam.
///
/// @notice An adapter implementing this interface vouches, for each fill in a sub-batch,
///         that `claimDigests[i]` is the correct answer for `requestDigests[i]`'s
///         predicate, and that `seal` is a valid attestation of the whole batch. Used by
///         classes whose underlying mechanism is naturally batched (e.g. an R0 STARK
///         over a merkle root of per-fill leaves).
///
/// @dev    Terminal seam. Classes with this `interfaceTag` are referenced by other
///         classes' `requiredAssessorClass` and MUST never be selected as a verifier
///         class — the router rejects this at `verifySubBatch`.
interface IBoundlessAssessor {
    /// @notice Verify the per-batch binding. `requestDigests.length == claimDigests.length`
    ///         is enforced by the caller (the router). Reverts on any mismatch.
    function verifyAssessor(
        bytes32[] calldata requestDigests,
        bytes32[] calldata claimDigests,
        bytes calldata assessorSeal
    ) external view;
}
