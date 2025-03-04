// Copyright (c) 2025 RISC Zero, Inc.
//
// All rights reserved.

pragma solidity ^0.8.20;

import {ReceiptClaim, ReceiptClaimLib} from "risc0/IRiscZeroVerifier.sol";
import {Seal, RiscZeroSetVerifier} from "risc0/RiscZeroSetVerifier.sol";
import {Selector} from "../src/types/Selector.sol";
import "../src/BoundlessMarket.sol";

library TestUtils {
    using ReceiptClaimLib for ReceiptClaim;

    function mockAssessor(
        Fulfillment[] memory fills,
        bytes32 assessorImageId,
        Selector[] memory selectors,
        address prover
    ) internal pure returns (ReceiptClaim memory) {
        bytes32[] memory claimDigests = new bytes32[](fills.length);
        bytes32[] memory requestDigests = new bytes32[](fills.length);
        for (uint256 i = 0; i < fills.length; i++) {
            claimDigests[i] = ReceiptClaimLib.ok(fills[i].imageId, sha256(fills[i].journal)).digest();
            requestDigests[i] = fills[i].requestDigest;
        }
        bytes32 root = MerkleProofish.processTree(claimDigests);

        bytes memory journal = abi.encode(
            AssessorJournal({requestDigests: requestDigests, root: root, selectors: selectors, prover: prover})
        );
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

    // keccak256("Permit(address owner,address spender,uint256 value,uint256 nonce,uint256 deadline)");
    bytes32 public constant PERMIT_TYPEHASH = 0x6e71edae12b1b97f4d1f60370fef10105fa2faae0126114a169c64845d6126c9;

    // computes the hash of a permit
    function getPermitHash(address owner, address spender, uint256 value, uint256 nonce, uint256 deadline)
        public
        pure
        returns (bytes32)
    {
        return keccak256(abi.encode(PERMIT_TYPEHASH, owner, spender, value, nonce, deadline));
    }

    /// @notice Adds a non-zero selector at the given index
    /// @dev Overwrites any existing selector at that index
    /// @param self The Selectors struct to modify
    /// @param index The index where to add the selector
    /// @param selector The selector to add
    function addSelector(Selector[] memory self, uint8 index, bytes4 selector)
        internal
        pure
        returns (Selector[] memory)
    {
        // Create a new array with one additional element.
        Selector[] memory newSelectors = new Selector[](self.length + 1);
        for (uint256 i = 0; i < self.length; i++) {
            newSelectors[i] = self[i];
        }
        newSelectors[self.length] = Selector(index, selector);
        return newSelectors;
    }
}
