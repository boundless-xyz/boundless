// Copyright (c) 2025 RISC Zero, Inc.
//
// All rights reserved.

pragma solidity ^0.8.20;

import "forge-std/Test.sol";
import {MerkleProofish} from "../../src/libraries/MerkleProofish.sol";

contract MerkleProofishTest is Test {
    function testProcessTree2() public pure {
        bytes32[] memory leaves = new bytes32[](2);
        leaves[0] = 0x6a428060b5d51f04583182f2ff1b565f9db661da12ee7bdc003e9ab6d5d91ba9;
        leaves[1] = 0x6a428060b5d51f04583182f2ff1b565f9db661da12ee7bdc003e9ab6d5d91ba9;

        bytes32 root = MerkleProofish.processTree(leaves);
        assertEq(root, 0x5032880539b5d039d4a4a8042745c9ad14934c96b76d7e61ea03550e29b234af);
    }

    function testProcessTree3() public pure {
        bytes32[] memory leaves = new bytes32[](3);
        leaves[0] = 0x6a428060b5d51f04583182f2ff1b565f9db661da12ee7bdc003e9ab6d5d91ba9;
        leaves[1] = 0x6a428060b5d51f04583182f2ff1b565f9db661da12ee7bdc003e9ab6d5d91ba9;
        leaves[2] = 0x6a428060b5d51f04583182f2ff1b565f9db661da12ee7bdc003e9ab6d5d91ba9;

        bytes32 root = MerkleProofish.processTree(leaves);
        assertEq(root, 0xe004c72e4cb697fa97669508df099edbc053309343772a25e56412fc7db8ebef);
    }
}
