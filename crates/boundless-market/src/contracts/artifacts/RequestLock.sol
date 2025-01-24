// Copyright (c) 2025 RISC Zero, Inc.
//
// All rights reserved.
// SPDX-License-Identifier: MIT
pragma solidity ^0.8.24;

using RequestLockLibrary for RequestLock global;

/// @notice Account state is a combination of the account balance, and locked and fulfilled flags for requests.
struct RequestLock {
    address prover;
    uint96 price;
    uint64 deadline;
    // Prover stake that may be taken if a proof is not delivered by the deadline.
    uint96 stake;
    /// @notice Keccak256 hash of the request, shortened to 64-bits. During fulfillment, this value is used
    /// to check that the request completed is the request that was locked, and not some other
    /// request with the same ID.
    /// @dev Note that this value is not collision resistant in that it is fairly easy to find two
    /// requests with the same fingerprint. However, requests much be signed to be valid, and so
    /// the existence of two valid requests with the same fingerprint requires either intention
    /// construction by the private key holder, which would be pointless, or accidental collision.
    /// With 64-bits, a client that constructed 65k signed requests with the same request ID would
    /// have a roughly 2^-32 chance of accidental collision, which is negligible in this scenario.
    ///
    /// This fingerprint binds the full request including e.g. the offer and input. Technically,
    /// all that is required is to bind the requirements. If there is some advantage to only binding
    /// the requirements here (e.g. less hashing costs) then that might be worth doing.
    ///
    /// There is another option here, which would be to have the request lock mapping index
    /// based on request digest instead of index. As a friction, this would introduce a second
    /// user-facing concept of what identifies a request.
    bytes8 fingerprint;
}

library RequestLockLibrary {}
