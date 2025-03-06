// Copyright (c) 2025 RISC Zero, Inc.
//
// All rights reserved.
pragma solidity ^0.8.20;

uint256 constant REQUEST_FLAGS_BITWIDTH = 2;
uint256 constant REQUEST_FLAGS_INITIAL_BITS = 64;

using AccountLibrary for Account global;

/// @title Account Struct and Library
/// @notice Represents the account state, including balance and request flags.
struct Account {
    /// @notice The balance of the account.
    /// @dev uint96 is enough to represent the entire token supply of Ether.
    uint96 balance;
    /// @dev Balance of staked tokens.
    uint96 stakeBalance;
    /// @notice 32 pairs of 2 bits representing the status of a request. One bit is for lock-in and
    /// the other is for fulfillment.
    /// @dev Request state flags are packed into a uint64 to make balance and flags for the first
    /// 32 requests fit in one slot.
    uint64 requestFlagsInitial;
    /// @dev Flags for the remaining requests are in a storage array.
    /// Each uint256 holds the packed flags for 128 requests, indexed in a linear fashion.
    /// Note that this struct cannot be instantiated in memory.
    uint256[(1 << 32) * REQUEST_FLAGS_BITWIDTH / 256] requestFlagsExtended;
}

library AccountLibrary {
    /// @notice Gets the locked and fulfilled request flags for the request with the given index.
    /// @param self The account to get the request flags from.
    /// @param idx The index of the request.
    /// @return locked True if the request is locked, false otherwise.
    /// @return fulfilled True if the request is fulfilled, false otherwise.
    function requestFlags(Account storage self, uint32 idx) internal view returns (bool locked, bool fulfilled) {
        if (idx < REQUEST_FLAGS_INITIAL_BITS / REQUEST_FLAGS_BITWIDTH) {
            uint64 masked = (
                self.requestFlagsInitial
                    & (uint64((1 << REQUEST_FLAGS_BITWIDTH) - 1) << uint64(idx * REQUEST_FLAGS_BITWIDTH))
            ) >> (idx * REQUEST_FLAGS_BITWIDTH);
            return (masked & uint64(1) != 0, masked & uint64(2) != 0);
        } else {
            uint256 idxShifted = idx - (REQUEST_FLAGS_INITIAL_BITS / REQUEST_FLAGS_BITWIDTH);
            uint256 packed = self.requestFlagsExtended[(idxShifted * REQUEST_FLAGS_BITWIDTH) / 256];
            uint256 maskShift = (idxShifted * REQUEST_FLAGS_BITWIDTH) % 256;
            uint256 masked = (packed & (uint256((1 << REQUEST_FLAGS_BITWIDTH) - 1) << maskShift)) >> maskShift;
            return (masked & uint256(1) != 0, masked & uint256(2) != 0);
        }
    }

    /// @notice Sets the locked and fulfilled request flags for the request with the given index.
    /// @dev The given value of flags will be applied with |= to the flags for the request. Least significant bit is locked, second-least significant is fulfilled.
    /// @param self The account to set the request flags for.
    /// @param idx The index of the request.
    /// @param flags The flags to set for the request.
    function setRequestFlags(Account storage self, uint32 idx, uint8 flags) internal {
        assert(flags < (1 << REQUEST_FLAGS_BITWIDTH));
        if (idx < REQUEST_FLAGS_INITIAL_BITS / REQUEST_FLAGS_BITWIDTH) {
            uint64 mask = uint64(flags) << uint64(idx * REQUEST_FLAGS_BITWIDTH);
            self.requestFlagsInitial |= mask;
        } else {
            uint256 idxShifted = idx - (REQUEST_FLAGS_INITIAL_BITS / REQUEST_FLAGS_BITWIDTH);
            uint256 mask = uint256(flags) << (uint256(idxShifted * REQUEST_FLAGS_BITWIDTH) % 256);
            self.requestFlagsExtended[(idxShifted * REQUEST_FLAGS_BITWIDTH) / 256] |= mask;
        }
    }

    /// @notice Sets the locked flag for the request with the given index.
    /// @dev The flag indicates that a request has been locked now or in the past.
    /// If a requests lock expires this flag will still be set.
    /// @param self The account to set the request flag for.
    /// @param idx The index of the request.
    function setRequestLocked(Account storage self, uint32 idx) internal {
        setRequestFlags(self, idx, 1);
    }

    /// @notice Sets the fulfilled flag for the request with the given index.
    /// @param self The account to set the request flag for.
    /// @param idx The index of the request.
    function setRequestFulfilled(Account storage self, uint32 idx) internal {
        setRequestFlags(self, idx, 2);
    }
}
