// Copyright (c) 2025 RISC Zero, Inc.
//
// All rights reserved.
pragma solidity ^0.8.20;

import {RequestId} from "./RequestId.sol";

using AssessorCommitmentLibrary for AssessorCommitment global;

/// @title Assessor Commitment Struct
/// @notice Represents the structured commitment used as a leaf in the Assessor guest Merkle tree guest.
struct AssessorCommitment {
    /// @notice The index of the request in the tree.
    uint256 index;
    /// @notice The request ID.
    RequestId id;
    /// @notice The request digest.
    bytes32 requestDigest;
    /// @notice The claim digest.
    bytes32 claimDigest;
}

library AssessorCommitmentLibrary {
    /// @dev Id is uint256 as for user defined types, the eip712 type hash uses the underlying type.
    string constant ASSESSOR_COMMITMENT_TYPE =
        "AssessorCommitment(uint256 index,uint256 id,bytes32 requestDigest,bytes32 claimDigest)";
    bytes32 constant ASSESSOR_COMMITMENT_TYPEHASH = keccak256(bytes(ASSESSOR_COMMITMENT_TYPE));

    /// @notice Computes the EIP-712 digest for the given commitment.
    /// @param commitment The commitment to compute the digest for.
    /// @return The EIP-712 digest of the commitment.
    function eip712Digest(AssessorCommitment memory commitment) internal pure returns (bytes32) {
        return keccak256(
            abi.encode(
                ASSESSOR_COMMITMENT_TYPEHASH,
                commitment.index,
                commitment.id,
                commitment.requestDigest,
                commitment.claimDigest
            )
        );
    }
}
