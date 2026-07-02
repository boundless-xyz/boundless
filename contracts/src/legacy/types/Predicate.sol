// Copyright 2026 Boundless Foundation, Inc.
//
// Use of this source code is governed by the Business Source License
// as found in the LICENSE-BSL file.
// SPDX-License-Identifier: BUSL-1.1
pragma solidity ^0.8.26;

import {ReceiptClaim, ReceiptClaimLib} from "risc0/IRiscZeroVerifier.sol";
import {Bytes} from "@openzeppelin/contracts/utils/Bytes.sol";

using PredicateLibrary for Predicate global;
using ReceiptClaimLib for ReceiptClaim;

/// @title Predicate Struct and Library
/// @notice A predicate is a function over the claim that determines whether it meets the clients requirements.
/// The data field is used to store the specific data associated with the predicate.
/// - DigestMatch: (bytes32, bytes32) -> abi.encodePacked(imageId, journalHash)
/// - PrefixMatch: (bytes32, bytes) -> abi.encodePacked(imageId, prefix)
/// - ClaimDigestMatch: (bytes32) -> abi.encode(claimDigest)
struct Predicate {
    PredicateType predicateType;
    bytes data;
}

enum PredicateType {
    DigestMatch,
    PrefixMatch,
    ClaimDigestMatch
}

library PredicateLibrary {
    string constant PREDICATE_TYPE = "Predicate(uint8 predicateType,bytes data)";
    bytes32 constant PREDICATE_TYPEHASH = keccak256(bytes(PREDICATE_TYPE));

    /// @notice Creates a digest match predicate.
    /// @param hash The hash to match.
    /// @return A Predicate struct with type DigestMatch and the provided hash.
    function createDigestMatchPredicate(bytes32 imageId, bytes32 hash) internal pure returns (Predicate memory) {
        return Predicate({predicateType: PredicateType.DigestMatch, data: abi.encodePacked(imageId, hash)});
    }

    /// @notice Creates a prefix match predicate.
    /// @param prefix The prefix to match.
    /// @return A Predicate struct with type PrefixMatch and the provided prefix.
    function createPrefixMatchPredicate(bytes32 imageId, bytes memory prefix) internal pure returns (Predicate memory) {
        return Predicate({predicateType: PredicateType.PrefixMatch, data: abi.encodePacked(imageId, prefix)});
    }

    /// @notice Creates a claim digest match predicate.
    /// @param claimDigest The claimDigest to match.
    /// @return A Predicate struct with type ClaimDigestMatch and the provided claimDigest.
    function createClaimDigestMatchPredicate(bytes32 claimDigest) internal pure returns (Predicate memory) {
        return Predicate({predicateType: PredicateType.ClaimDigestMatch, data: abi.encodePacked(claimDigest)});
    }

    /// @notice Evaluates the predicate against the image ID and journal.
    /// @dev If the predicate is of type ClaimDigestMatch and image ID and journal are not available,
    /// use the evaluation function with the claim digest instead.
    /// @param predicate The predicate to evaluate.
    /// @param imageId Image ID to use for evaluation.
    /// @param journal The journal to evaluate against.
    /// @return True if the predicate is satisfied, false otherwise.
    function eval(Predicate memory predicate, bytes32 imageId, bytes memory journal) internal pure returns (bool) {
        if (predicate.predicateType == PredicateType.DigestMatch) {
            require(predicate.data.length == 64, "Invalid DigestMatch data length");
            bytes memory dataJournal = Bytes.slice(predicate.data, 32);
            return bytes32(dataJournal) == sha256(abi.encode(journal)) && bytes32(predicate.data) == imageId;
        } else if (predicate.predicateType == PredicateType.PrefixMatch) {
            require(predicate.data.length >= 32, "Invalid PrefixMatch data length");
            bytes memory dataJournal = Bytes.slice(predicate.data, 32);
            return startsWith(journal, dataJournal) && bytes32(predicate.data) == imageId;
        } else if (predicate.predicateType == PredicateType.ClaimDigestMatch) {
            require(predicate.data.length == 32, "Invalid ClaimDigestMatch data length");
            return bytes32(predicate.data) == ReceiptClaimLib.ok(imageId, sha256(abi.encode(journal))).digest();
        } else {
            revert("Unreachable code");
        }
    }

    /// @notice Evaluates the predicate against the claim digest.
    /// @dev This function should be used when the predicate is of type ClaimDigestMatch
    /// and the image ID and journal are not available.
    /// @param predicate The predicate to evaluate.
    /// @param claimDigest Claim digest to use for evaluation.
    /// @return True if the predicate is satisfied, false otherwise.
    function eval(Predicate memory predicate, bytes32 claimDigest) internal pure returns (bool) {
        if (predicate.predicateType == PredicateType.ClaimDigestMatch) {
            require(predicate.data.length == 32, "Invalid ClaimDigestMatch data length");
            return bytes32(predicate.data) == claimDigest;
        } else {
            revert("Predicate not of type ClaimDigestMatch");
        }
    }

    /// @notice Checks if the journal starts with the given prefix.
    /// @param journal The journal to check.
    /// @param prefix The prefix to check for.
    /// @return True if the journal starts with the prefix, false otherwise.
    function startsWith(bytes memory journal, bytes memory prefix) internal pure returns (bool) {
        if (journal.length < prefix.length) {
            return false;
        }
        if (prefix.length == 0) {
            return true;
        }
        bytes memory slice = new bytes(prefix.length);
        assembly {
            let dest := add(slice, 0x20)
            let src := add(journal, 0x20)
            for { let i := 0 } lt(i, mload(prefix)) { i := add(i, 0x20) } { mstore(add(dest, i), mload(add(src, i))) }
        }
        return keccak256(slice) == keccak256(prefix);
    }

    /// @notice Computes the EIP-712 digest for the given predicate.
    /// @param predicate The predicate to compute the digest for.
    /// @return The EIP-712 digest of the predicate.
    function eip712Digest(Predicate memory predicate) internal pure returns (bytes32) {
        return keccak256(abi.encode(PREDICATE_TYPEHASH, predicate.predicateType, keccak256(predicate.data)));
    }
}
