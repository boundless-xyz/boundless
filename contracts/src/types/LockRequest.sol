// Copyright (c) 2025 RISC Zero, Inc.
//
// All rights reserved.
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

/// @title Proof Request Struct and Library
/// @notice Represents a proof request with its associated data and functions.
struct LockRequest {
    /// @notice Unique ID for this request, constructed from the client address and a 32-bit index.
    ProofRequest request;
}

library LockRequestLibrary {
    /// @dev Id is uint256 as for user defined types, the eip712 type hash uses the underlying type.
    string constant LOCK_REQUEST_TYPE =
        "LockRequest(ProofRequest request)";

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
        return keccak256(
            abi.encode(
                LOCK_REQUEST_TYPEHASH,
                lockRequest.request.eip712Digest()
            )
        );
    }

    /// @notice Computes the EIP-712 digest for the given lock request.
    /// @param proofRequestEip712Digest The EIP-712 digest of the proof request.
    /// @return The EIP-712 digest of the lock request.
    function eip712DigestFromPrecomputedDigest(bytes32 proofRequestEip712Digest) internal pure returns (bytes32) {
        return keccak256(
            abi.encode(
                LOCK_REQUEST_TYPEHASH,
                proofRequestEip712Digest
            )
        );
    }
}
