// Copyright 2026 Boundless Foundation, Inc.
//
// Use of this source code is governed by the Business Source License
// as found in the LICENSE-BSL file.
// SPDX-License-Identifier: BUSL-1.1

pragma solidity ^0.8.26;

/// @title IBoundlessJointVerifierAssessor — per-fill combined verifier + binding.
///
/// @notice An adapter implementing this interface vouches in one call for the
///         cryptographic check on `seal`, the binding between `requestDigest` and
///         `claimDigest`, AND the binding to `prover` (the address the market will
///         credit / slash). Used by classes whose underlying mechanism is naturally
///         per-fill, where splitting the check across two seams would waste a
///         dispatch and force batched signing.
///
/// @dev    `prover` is forwarded by the router as a universal arg, identical across
///         every fill in the sub-batch. Each adapter is responsible for binding it via
///         its own mechanism — a signature-based adapter would include `prover` in the
///         signing payload alongside `(requestDigest, claimDigest)`. The market trusts
///         the adapter to have verified the binding.
///
///         Classes whose `interfaceTag == type(IBoundlessJointVerifierAssessor).interfaceId`
///         do NOT carry an assessor seam — `requiredAssessorClass` must be 0x00 and the
///         router skips the per-batch assessor call for sub-batches under such classes.
interface IBoundlessJointVerifierAssessor {
    /// @notice Verify, for one fill, that `seal` cryptographically attests `claimDigest`,
    ///         that `claimDigest` is the correct binding for `requestDigest`, and that
    ///         `seal` also commits to `prover`. Reverts on failure.
    function verifyJoint(bytes32 requestDigest, bytes32 claimDigest, address prover, bytes calldata seal) external view;
}
