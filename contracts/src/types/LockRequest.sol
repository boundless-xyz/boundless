// Copyright 2025 RISC Zero, Inc.
//
// Use of this source code is governed by the Business Source License
// as found in the LICENSE-BSL file.
pragma solidity ^0.8.20;

import {ProofRequest, ProofRequestLibrary} from "./ProofRequest.sol";
import {Account} from "./Account.sol";
import {Callback, CallbackLibrary} from "./Callback.sol";
import {Offer, OfferLibrary} from "./Offer.sol";
import {Predicate, PredicateLibrary} from "./Predicate.sol";
import {Input, InputType, InputLibrary} from "./Input.sol";
import {Requirements, RequirementsLibrary} from "./Requirements.sol";
import {ECDSA} from "@openzeppelin/contracts/utils/cryptography/ECDSA.sol";
import {IBoundlessMarket} from "../IBoundlessMarket.sol";
import {IERC1271} from "@openzeppelin/contracts/interfaces/IERC1271.sol";

using LockRequestLibrary for LockRequest global;

/// @title Lock Request Struct and Library
/// @notice Message sent by a prover to indicate that they intend to lock the given request.
struct LockRequest {
    /// @notice The proof request that the prover is locking.
    ProofRequest request;
}

library LockRequestLibrary {
    string constant LOCK_REQUEST_TYPE = "LockRequest(ProofRequest request)";

    bytes32 constant LOCK_REQUEST_TYPEHASH = keccak256(
        abi.encodePacked(
            LOCK_REQUEST_TYPE,
            CallbackLibrary.CALLBACK_TYPE,
            InputLibrary.INPUT_TYPE,
            OfferLibrary.OFFER_TYPE,
            PredicateLibrary.PREDICATE_TYPE,
            ProofRequestLibrary.PROOF_REQUEST_TYPE,
            RequirementsLibrary.REQUIREMENTS_TYPE
        )
    );

    /// @notice Computes the EIP-712 digest for the given lock request.
    /// @param lockRequest The lock request to compute the digest for.
    /// @return The EIP-712 digest of the lock request.
    function eip712Digest(LockRequest memory lockRequest) internal pure returns (bytes32) {
        return keccak256(abi.encode(LOCK_REQUEST_TYPEHASH, lockRequest.request.eip712Digest()));
    }

    /// @notice Computes the EIP-712 digest for the given lock request from a precomputed EIP-712 proof request digest.
    /// @dev This avoids recomputing the proof request digest in the case where the proof request digest has already been computed.
    /// @param proofRequestEip712Digest The EIP-712 digest of the proof request.
    /// @return The EIP-712 digest of the lock request.
    function eip712DigestFromPrecomputedDigest(bytes32 proofRequestEip712Digest) internal pure returns (bytes32) {
        return keccak256(abi.encode(LOCK_REQUEST_TYPEHASH, proofRequestEip712Digest));
    }
}
