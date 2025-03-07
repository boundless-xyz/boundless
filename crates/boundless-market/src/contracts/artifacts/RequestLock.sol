// Copyright (c) 2025 RISC Zero, Inc.
//
// All rights reserved.
pragma solidity ^0.8.24;

using RequestLockLibrary for RequestLock global;

/// @notice Stores information about requests that have been locked.
/// @dev RequestLock is an internal structure that is modified at various points in the proof lifecycle.
/// Fields can be valid or invalid depending where in the lifecycle we are. Integrators should not rely on RequestLock
/// for determining the status of a request. Instead, they shouldalways use BoundlessMarket's view functions.
///
/// Packed to fit into 2 slots.
struct RequestLock {
    ///
    /// Storage slot 1
    ///
    /// @notice The address of the prover that locked the request _or_ the address of the prover that fulfilled the request.
    address prover;
    /// @notice The final timestamp at which the locked request can be fulfilled for payment by the locker.
    uint64 lockDeadline;
    /// @notice The number of seconds from the lockDeadline to where the request expires.
    /// @dev Represented as a delta so that it can be packed into 2 slots.
    uint24 deadlineDelta;
    /// @notice Flags that indicate the state of the request lock.
    uint8 requestLockFlags;
    ///
    /// Storage slot 2
    ///
    /// @notice The price that the prover will be paid for fulfilling the request.
    uint96 price;
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

library RequestLockLibrary {
    uint8 internal constant PROVER_PAID_DURING_LOCK_FLAG = 1 << 0;
    uint8 internal constant PROVER_PAID_AFTER_LOCK_FLAG = 1 << 1;
    uint8 internal constant SLASHED_FLAG = 1 << 2;

    /// @notice Calculates the deadline for the locked request.
    /// @param requestLock The request lock to calculate the deadline for.
    /// @return The deadline for the request.
    function deadline(RequestLock memory requestLock) internal pure returns (uint64) {
        return requestLock.lockDeadline + requestLock.deadlineDelta;
    }

    function setProverPaidBeforeLockDeadline(RequestLock storage requestLock) internal {
        requestLock.requestLockFlags = PROVER_PAID_DURING_LOCK_FLAG;
        // Zero out second slot for gas refund.
        requestLock.price = uint96(0);
        requestLock.stake = uint96(0);
        requestLock.fingerprint = bytes8(0);
    }

    function setProverPaidAfterLockDeadline(RequestLock storage requestLock, address prover) internal {
        requestLock.prover = prover;
        requestLock.requestLockFlags = PROVER_PAID_AFTER_LOCK_FLAG;
        // We don't zero out the second slot as stake information is required for slashing.
    }

    function setSlashed(RequestLock storage requestLock) internal {
        requestLock.requestLockFlags |= SLASHED_FLAG;
        // Zero out second slot for gas refund.
        requestLock.price = uint96(0);
        requestLock.stake = uint96(0);
        requestLock.fingerprint = bytes8(0);
    }

    function hasBeenLocked(RequestLock memory requestLock) internal pure returns (bool) {
        return requestLock.prover != address(0);
    }

    /// @notice Returns true if the request was fulfilled by the locker
    /// before the lock deadline and they have been paid.
    /// @param requestLock The request lock to check.
    /// @return True if the request was fulfilled before the lock deadline and the prover was paid, false otherwise.
    function isProverPaidBeforeLockDeadline(RequestLock memory requestLock) internal pure returns (bool) {
        return requestLock.requestLockFlags & PROVER_PAID_DURING_LOCK_FLAG != 0;
    }

    /// @notice Checks if the request was fulfilled by any prover after the lock deadline.
    /// @param requestLock The request lock to check.
    /// @return True if the request is fulfilled after the lock deadline and the prover was paid, false otherwise.
    function isProverPaidAfterLockDeadline(RequestLock memory requestLock) internal pure returns (bool) {
        return requestLock.requestLockFlags & PROVER_PAID_AFTER_LOCK_FLAG != 0;
    }

    /// @notice Checks if the locked request was fulfilled and _a_ prover was paid. The prover paid
    /// could be the prover that locked, or a prover that filled after the lock deadline.
    /// @param requestLock The request lock to check.
    /// @return True if the request is fulfilled after the lock deadline, false otherwise.
    function isProverPaid(RequestLock memory requestLock) internal pure returns (bool) {
        return isProverPaidBeforeLockDeadline(requestLock) || isProverPaidAfterLockDeadline(requestLock);
    }

    /// @notice Checks if the request was slashed.
    /// @dev Whether a request resulted in a slash does not indicate whether the request was fulfilled
    /// since it is possible for a request to be fulfilled after a request lock has expired.
    /// @param requestLock The request lock to check.
    /// @return True if the request is slashed, false otherwise.
    function isSlashed(RequestLock memory requestLock) internal pure returns (bool) {
        return requestLock.requestLockFlags & SLASHED_FLAG != 0;
    }
}
