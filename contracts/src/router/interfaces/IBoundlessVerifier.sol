// Copyright 2026 Boundless Foundation, Inc.
//
// Use of this source code is governed by the Business Source License
// as found in the LICENSE-BSL file.
// SPDX-License-Identifier: BUSL-1.1

pragma solidity ^0.8.26;

/// @title IBoundlessVerifier — per-fill cryptographic verifier seam.
///
/// @notice An adapter implementing this interface vouches that `seal` cryptographically
///         attests `claimDigest`. It does NOT see `requestDigest` and therefore CANNOT
///         speak to whether the proof satisfies the requestor's predicate — that binding
///         is the assessor's job, dispatched separately by the router.
///
/// @dev    Classes whose `interfaceTag == type(IBoundlessVerifier).interfaceId`
///         must declare a non-zero `requiredAssessorClass` in their metadata.
interface IBoundlessVerifier {
    /// @notice Verify the seal cryptographically. Reverts on failure.
    function verify(bytes calldata seal, bytes32 claimDigest) external view;
}
