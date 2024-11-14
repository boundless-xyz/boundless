// Copyright (c) 2024 RISC Zero, Inc.
//
// All rights reserved.

pragma solidity ^0.8.20;

import {ReceiptClaim, ReceiptClaimLib} from "risc0/IRiscZeroVerifier.sol";
import {Seal, RiscZeroSetVerifier} from "../src/RiscZeroSetVerifier.sol";
import "../src/ProofMarket.sol";

// Functions copied from OZ MerkleProof library to allow building the Merkle tree above.
// TODO(victor): Drop this library.
library MerkleProofish {
    // Compute the root of the Merkle tree given all of its leaves.
    // Assumes that the array of leaves is no longer needed, and can be overwritten.
    function processTree(bytes32[] memory leaves) internal pure returns (bytes32 root) {
        require(leaves.length > 0, "Leaves array must contain at least one element");

        // If there's only one leaf, the root is the leaf itself
        if (leaves.length == 1) {
            return leaves[0];
        }

        uint256 n = leaves.length;

        // Process the leaves array in pairs, iteratively computing the hash of each pair
        while (n > 1) {
            uint256 nextLevelLength = (n + 1) / 2; // Upper bound of next level (handles odd number of elements)

            // Hash the current level's pairs and place results at the start of the array
            for (uint256 i = 0; i < n / 2; i++) {
                leaves[i] = _hashPair(leaves[2 * i], leaves[2 * i + 1]);
            }

            // If there's an odd number of elements, propagate the last element directly
            if (n % 2 == 1) {
                leaves[n / 2] = leaves[n - 1];
            }

            // Move to the next level (the computed hashes are now the new "leaves")
            n = nextLevelLength;
        }

        // The root is now the single element left in the array
        root = leaves[0];
    }

    /**
     * @dev Sorts the pair (a, b) and hashes the result.
     */
    function _hashPair(bytes32 a, bytes32 b) internal pure returns (bytes32) {
        return a < b ? _efficientHash(a, b) : _efficientHash(b, a);
    }

    /**
     * @dev Implementation of keccak256(abi.encode(a, b)) that doesn't allocate or expand memory.
     */
    function _efficientHash(bytes32 a, bytes32 b) private pure returns (bytes32 value) {
        /// @solidity memory-safe-assembly
        assembly {
            mstore(0x00, a)
            mstore(0x20, b)
            value := keccak256(0x00, 0x40)
        }
    }
}

library TestUtils {
    using ReceiptClaimLib for ReceiptClaim;

    function mockAssessor(
        Fulfillment[] memory fills,
        bytes32 assessorImageId,
        bytes32 eip712DomainSeparator,
        address prover
    ) internal pure returns (ReceiptClaim memory) {
        bytes32[] memory claimDigests = new bytes32[](fills.length);
        uint192[] memory ids = new uint192[](fills.length);
        for (uint256 i = 0; i < fills.length; i++) {
            claimDigests[i] = ReceiptClaimLib.ok(fills[i].imageId, sha256(fills[i].journal)).digest();
            ids[i] = fills[i].id;
        }

        bytes memory journal =
            abi.encode(AssessorJournal({requestIds: ids, eip712DomainSeparator: eip712DomainSeparator, prover: prover}));
        return ReceiptClaimLib.ok(assessorImageId, sha256(journal));
    }

    function mockAssessorSeal(RiscZeroSetVerifier setVerifier, bytes32 claimDigest)
        internal
        view
        returns (bytes memory)
    {
        bytes32[] memory path = new bytes32[](1);
        path[0] = claimDigest;
        return encodeSeal(setVerifier, Proof({siblings: path}));
    }

    function mockSetBuilder(Fulfillment[] memory fills)
        internal
        pure
        returns (bytes32 batchRoot, bytes32[][] memory tree)
    {
        bytes32[] memory claimDigests = new bytes32[](fills.length);
        for (uint256 i = 0; i < fills.length; i++) {
            claimDigests[i] = ReceiptClaimLib.ok(fills[i].imageId, sha256(fills[i].journal)).digest();
        }
        // compute the merkle tree of the batch
        (batchRoot, tree) = computeMerkleTree(claimDigests);
    }

    function fillInclusionProofs(
        RiscZeroSetVerifier setVerifier,
        Fulfillment[] memory fills,
        bytes32 assessorDigest,
        bytes32[][] memory tree
    ) internal view {
        // generate inclusion proofs for each claim
        Proof[] memory proofs = computeProofs(tree);

        for (uint256 i = 0; i < fills.length; i++) {
            fills[i].seal = encodeSeal(setVerifier, append(proofs[i], assessorDigest));
        }
    }

    struct Proof {
        bytes32[] siblings;
    }

    // Build the Merkle Tree and return the root and the entire tree structure
    function computeMerkleTree(bytes32[] memory leaves) internal pure returns (bytes32 root, bytes32[][] memory tree) {
        require(leaves.length > 0, "Leaves list is empty, cannot compute Merkle root");

        // Calculate the height of the tree (number of levels)
        uint256 numLevels = log2Ceil(leaves.length) + 1;

        // Initialize the tree structure
        tree = new bytes32[][](numLevels);
        tree[0] = leaves;

        uint256 currentLevelSize = leaves.length;

        // Build the tree level by level
        for (uint256 level = 0; currentLevelSize > 1; level++) {
            uint256 nextLevelSize = (currentLevelSize + 1) / 2;
            tree[level + 1] = new bytes32[](nextLevelSize);

            for (uint256 i = 0; i < nextLevelSize; i++) {
                uint256 leftIndex = i * 2;
                uint256 rightIndex = leftIndex + 1;

                bytes32 leftHash = tree[level][leftIndex];
                if (rightIndex < currentLevelSize) {
                    bytes32 rightHash = tree[level][rightIndex];

                    tree[level + 1][i] = MerkleProofish._hashPair(leftHash, rightHash);
                } else {
                    // If the node has no right sibling, copy it up to the next level.
                    tree[level + 1][i] = leftHash;
                }
            }

            currentLevelSize = nextLevelSize;
        }

        root = tree[tree.length - 1][0];
    }

    function computeProofs(bytes32[][] memory tree) internal pure returns (Proof[] memory proofs) {
        uint256 numLeaves = tree[0].length;
        uint256 proofLength = tree.length - 1; // Maximum possible length of the proof
        proofs = new Proof[](numLeaves);

        // Generate proof for each leaf
        for (uint256 leafIndex = 0; leafIndex < numLeaves; leafIndex++) {
            bytes32[] memory tempSiblings = new bytes32[](proofLength);
            uint256 actualProofLength = 0;
            uint256 index = leafIndex;

            // Collect the siblings for the proof
            for (uint256 level = 0; level < tree.length - 1; level++) {
                uint256 siblingIndex = (index % 2 == 0) ? index + 1 : index - 1;

                if (siblingIndex < tree[level].length) {
                    tempSiblings[actualProofLength] = tree[level][siblingIndex];
                    actualProofLength++;
                }

                index /= 2;
            }

            // Adjust the length of the proof to exclude any unused slots
            proofs[leafIndex].siblings = new bytes32[](actualProofLength);
            for (uint256 i = 0; i < actualProofLength; i++) {
                proofs[leafIndex].siblings[i] = tempSiblings[i];
            }
        }
    }

    function encodeSeal(RiscZeroSetVerifier setVerifier, TestUtils.Proof memory merkleProof, bytes memory rootSeal)
        internal
        view
        returns (bytes memory)
    {
        return abi.encodeWithSelector(setVerifier.SELECTOR(), Seal({path: merkleProof.siblings, rootSeal: rootSeal}));
    }

    function encodeSeal(RiscZeroSetVerifier setVerifier, TestUtils.Proof memory merkleProof)
        internal
        view
        returns (bytes memory)
    {
        bytes memory rootSeal;
        return encodeSeal(setVerifier, merkleProof, rootSeal);
    }

    function append(Proof memory proof, bytes32 newNode) internal pure returns (Proof memory) {
        bytes32[] memory newSiblings = new bytes32[](proof.siblings.length + 1);
        for (uint256 i = 0; i < proof.siblings.length; i++) {
            newSiblings[i] = proof.siblings[i];
        }
        newSiblings[proof.siblings.length] = newNode;
        proof.siblings = newSiblings;
        return proof;
    }

    function log2Ceil(uint256 x) private pure returns (uint256) {
        uint256 res = 0;
        uint256 value = x;
        while (value > 1) {
            value = (value + 1) / 2;
            res += 1;
        }
        return res;
    }
}
