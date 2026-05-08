// Copyright 2026 Boundless Foundation, Inc.
//
// Use of this source code is governed by the Business Source License
// as found in the LICENSE-BSL file.
// SPDX-License-Identifier: BUSL-1.1

pragma solidity ^0.8.26;

/// @title IBoundlessJointVerifierAssessor — per-fill combined verifier + binding.
///
/// @notice An adapter implementing this interface vouches in one call for both the
///         cryptographic check on `seal` AND the binding between `requestDigest` and
///         `claimDigest` (e.g. an EIP-712 signature over the joint hash). Used by classes
///         whose underlying mechanism is naturally per-fill, where splitting the check
///         across two seams would waste a dispatch and force batched signing.
///
/// @dev    Classes whose `interfaceTag == type(IBoundlessJointVerifierAssessor).interfaceId`
///         do NOT carry an assessor seam — `requiredAssessorClass` must be 0x00 and the
///         router skips the per-batch assessor call for sub-batches under such classes.
interface IBoundlessJointVerifierAssessor {
    /// @notice Verify, for one fill, that `seal` cryptographically attests `claimDigest`
    ///         AND that `claimDigest` is the correct binding for `requestDigest`. Reverts
    ///         on failure.
    function verifyJoint(bytes32 requestDigest, bytes32 claimDigest, bytes calldata seal) external view;
}
