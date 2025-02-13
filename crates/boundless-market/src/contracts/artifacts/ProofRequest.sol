// Copyright (c) 2025 RISC Zero, Inc.
//
// All rights reserved.
pragma solidity ^0.8.20;

import {RequestId} from "./RequestId.sol";
import {Account} from "./Account.sol";
import {Offer, OfferLibrary} from "./Offer.sol";
import {Predicate, PredicateLibrary} from "./Predicate.sol";
import {Input, InputType, InputLibrary} from "./Input.sol";
import {Requirements, RequirementsLibrary} from "./Requirements.sol";
import {ECDSA} from "@openzeppelin/contracts/utils/cryptography/ECDSA.sol";
import {IBoundlessMarket} from "../IBoundlessMarket.sol";

using ProofRequestLibrary for ProofRequest global;

/// @title Proof Request Struct and Library
/// @notice Represents a proof request with its associated data and functions.
struct ProofRequest {
    /// @notice Unique ID for this request, constructed from the client address and a 32-bit index.
    RequestId id;
    /// @notice Requirements of the delivered proof.
    /// @dev Specifies the program that must be run, and constrains the value of the journal, specifying the statement that is requesting to be proven.
    Requirements requirements;
    /// @notice A public URI where the program (i.e. image) can be downloaded.
    /// @dev This URI will be accessed by provers that are evaluating whether to bid on the request.
    string imageUrl;
    /// @notice Input to be provided to the zkVM guest execution.
    Input input;
    /// @notice Offer specifying how much the client is willing to pay to have this request fulfilled.
    Offer offer;
}

library ProofRequestLibrary {
    /// @dev Id is uint256 as for user defined types, the eip712 type hash uses the underlying type.
    string constant PROOF_REQUEST_TYPE =
        "ProofRequest(uint256 id,Requirements requirements,string imageUrl,Input input,Offer offer)";
    bytes32 constant PROOF_REQUEST_TYPEHASH = keccak256(
        abi.encodePacked(
            PROOF_REQUEST_TYPE,
            InputLibrary.INPUT_TYPE,
            OfferLibrary.OFFER_TYPE,
            PredicateLibrary.PREDICATE_TYPE,
            RequirementsLibrary.REQUIREMENTS_TYPE
        )
    );

    /// @notice Computes the EIP-712 digest for the given proof request.
    /// @param request The proof request to compute the digest for.
    /// @return The EIP-712 digest of the proof request.
    function eip712Digest(ProofRequest memory request) internal pure returns (bytes32) {
        return keccak256(
            abi.encode(
                PROOF_REQUEST_TYPEHASH,
                request.id,
                request.requirements.eip712Digest(),
                keccak256(bytes(request.imageUrl)),
                request.input.eip712Digest(),
                request.offer.eip712Digest()
            )
        );
    }

    /// @notice Verifies the client's signature over the proof request.
    /// @param structHash The EIP-712 struct hash of the proof request.
    /// @param addr The address of the client.
    /// @param signature The signature to validate.
    /// @return The struct hash if the signature is valid.
    function verifyClientSignature(ProofRequest calldata, bytes32 structHash, address addr, bytes calldata signature)
        internal
        pure
        returns (bytes32)
    {
        if (ECDSA.recover(structHash, signature) != addr) {
            revert IBoundlessMarket.InvalidSignature();
        }
        return structHash;
    }

    /// @notice Extracts the prover's signature for the given proof request.
    /// @param structHash The EIP-712 struct hash of the proof request.
    /// @param proverSignature The prover's signature to extract.
    /// @return The address of the prover.
    function extractProverSignature(ProofRequest calldata, bytes32 structHash, bytes calldata proverSignature)
        internal
        pure
        returns (address)
    {
        return ECDSA.recover(structHash, proverSignature);
    }

    /// @notice Validates the proof request.
    /// @param request The proof request to validate.
    /// @param accounts The mapping of accounts.
    /// @param client The address of the client.
    /// @param idx The index of the request.
    /// @return lockDeadline1 The deadline for when a lock expires for the request.
    /// @return deadline1 The deadline for the request as a whole.
    function validateRequest(
        ProofRequest calldata request,
        mapping(address => Account) storage accounts,
        address client,
        uint32 idx
    ) internal view returns (uint64 lockDeadline1, uint64 deadline1) {
        (lockDeadline1, deadline1) = request.offer.validate(request.id);

        // Check that the request is not already locked or fulfilled.
        // TODO: Currently these checks are run here as part of the priceRequest path.
        // this may be redundant, because we must also check them during fulfillment. Should
        // these checks be moved from this method to _lockRequestAuthed?
        (bool locked, bool fulfilled) = accounts[client].requestFlags(idx);
        if (locked) {
            revert IBoundlessMarket.RequestIsLocked({requestId: request.id});
        }
        if (fulfilled) {
            revert IBoundlessMarket.RequestIsFulfilled({requestId: request.id});
        }
    }
}
