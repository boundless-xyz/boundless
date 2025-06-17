// Copyright 2025 RISC Zero, Inc.
//
// Use of this source code is governed by the Business Source License
// as found in the LICENSE-BSL file.
pragma solidity ^0.8.24;

import {Test} from "forge-std/Test.sol";
import {
    IRiscZeroVerifier, Receipt as RiscZeroReceipt, ReceiptClaim, ReceiptClaimLib
} from "risc0/IRiscZeroVerifier.sol";
import {BoundlessMarketCallback} from "../src/BoundlessMarketCallback.sol";

// Test implementation of BoundlessMarketCallback
contract TestCallback is BoundlessMarketCallback {
    event ProofHandled(bytes32 imageId, bytes journal, bytes seal);

    constructor(IRiscZeroVerifier verifier, address boundlessMarket, bytes32 imageId)
        BoundlessMarketCallback(verifier, boundlessMarket, imageId)
    {}

    function _handleProof(bytes32 imageId, bytes calldata journal, bytes calldata seal) internal override {
        emit ProofHandled(imageId, journal, seal);
    }
}

contract MockRiscZeroVerifier is IRiscZeroVerifier {
    function verify(bytes calldata seal, bytes32 imageId, bytes32 claimDigest) public view {}
    function verifyIntegrity(RiscZeroReceipt calldata receipt) public view {}
}

contract BoundlessMarketCallbackTest is Test {
    using ReceiptClaimLib for ReceiptClaim;

    MockRiscZeroVerifier public verifier;
    TestCallback public callback;
    address public boundlessMarket;

    bytes32 constant TEST_IMAGE_ID = bytes32(uint256(1));
    bytes constant TEST_JOURNAL = "test journal";
    bytes constant TEST_SEAL = "test seal";

    function setUp() public {
        verifier = new MockRiscZeroVerifier();
        boundlessMarket = makeAddr("boundlessMarket");
        callback = new TestCallback(verifier, boundlessMarket, TEST_IMAGE_ID);
    }

    function testHandleProof() public {
        vm.expectEmit(true, true, true, true);
        emit TestCallback.ProofHandled(TEST_IMAGE_ID, TEST_JOURNAL, TEST_SEAL);

        // Expect a call to verify with the correct parameters
        bytes32 expectedJournalDigest = ReceiptClaimLib.ok(TEST_IMAGE_ID, sha256(TEST_JOURNAL)).digest();

        vm.prank(boundlessMarket);
        vm.expectCall(
            address(verifier),
            abi.encodeCall(IRiscZeroVerifier.verifyIntegrity, (RiscZeroReceipt(TEST_SEAL, expectedJournalDigest)))
        );
        callback.handleProof(TEST_IMAGE_ID, TEST_JOURNAL, TEST_SEAL);
    }

    function testHandleProofIncorrectCaller() public {
        vm.prank(makeAddr("other"));
        vm.expectRevert("Invalid sender");
        callback.handleProof(TEST_IMAGE_ID, TEST_JOURNAL, TEST_SEAL);
    }

    function testHandleProofIncorrectImageId() public {
        vm.prank(boundlessMarket);
        vm.expectRevert("Invalid Image ID");
        callback.handleProof(bytes32(uint256(99)), TEST_JOURNAL, TEST_SEAL);
    }
}
