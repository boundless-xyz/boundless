// Copyright 2025 RISC Zero, Inc.
//
// Use of this source code is governed by the Business Source License
// as found in the LICENSE-BSL file.
pragma solidity ^0.8.20;

import {IBoundlessMarket} from "../IBoundlessMarket.sol";

library MerkleProofish {
    // Compute the root of the Merkle tree given all of its leaves.
    // Assumes that the array of leaves is no longer needed, and can be overwritten.
    function processTree(bytes32[] memory leaves) internal pure returns (bytes32 root) {
        if (leaves.length == 0) {
            revert IBoundlessMarket.InvalidRequest();
        }

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
                leaves[nextLevelLength - 1] = leaves[n - 1];
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
