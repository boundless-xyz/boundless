// SPDX-License-Identifier: BUSL-1.1
pragma solidity ^0.8.24;

import {Test} from "forge-std/Test.sol";
import {IRiscZeroVerifier, Receipt, ReceiptClaim, ReceiptClaimLib} from "risc0/IRiscZeroVerifier.sol";
import {BoundlessMarketCallback} from "../src/BoundlessMarketCallback.sol";

contract MockRiscZeroVerifier is IRiscZeroVerifier {
    uint256 public verifyCallCount;
    bytes32 public lastImageId;
    bytes32 public lastClaimDigest;

    function verify(bytes calldata seal, bytes32 imageId, bytes32 claimDigest) external {
        verifyCallCount++;
        lastImageId = imageId;
        lastClaimDigest = claimDigest;
    }
}

// Test implementation of BoundlessMarketCallback
contract TestCallback is BoundlessMarketCallback {
    event ProofHandled(bytes32 imageId, bytes journal, bytes seal);

    constructor(IRiscZeroVerifier verifier, address boundlessMarket)
        BoundlessMarketCallback(verifier, boundlessMarket)
    {}

    function _handleProof(bytes32 imageId, bytes calldata journal, bytes calldata seal) internal override {
        emit ProofHandled(imageId, journal, seal);
    }
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
        callback = new TestCallback(verifier, boundlessMarket);
    }

    function test_handleProof_fromBoundlessMarket() public {
        // When called from BoundlessMarket, verification should be skipped
        vm.prank(boundlessMarket);

        vm.expectEmit(true, true, true, true);
        emit TestCallback.ProofHandled(TEST_IMAGE_ID, TEST_JOURNAL, TEST_SEAL);

        callback.handleProof(TEST_IMAGE_ID, TEST_JOURNAL, TEST_SEAL);

        // Verify that the verifier was not called
        assertEq(verifier.verifyCallCount(), 0);
        assertEq(verifier.lastImageId(), bytes32(0));
        assertEq(verifier.lastClaimDigest(), bytes32(0));
    }

    function test_handleProof_fromOtherAddress() public {
        address caller = makeAddr("caller");
        vm.prank(caller);

        vm.expectEmit(true, true, true, true);
        emit TestCallback.ProofHandled(TEST_IMAGE_ID, TEST_JOURNAL, TEST_SEAL);

        callback.handleProof(TEST_IMAGE_ID, TEST_JOURNAL, TEST_SEAL);

        // Verify that the verifier was called exactly once
        assertEq(verifier.verifyCallCount(), 1);

        // Verify correct parameters were passed
        assertEq(verifier.lastImageId(), TEST_IMAGE_ID);
        bytes32 expectedClaimDigest = ReceiptClaimLib.ok(TEST_IMAGE_ID, sha256(TEST_JOURNAL)).digest();
        assertEq(verifier.lastClaimDigest(), expectedClaimDigest);
    }

    function test_handleProof_multipleCallsFromDifferentSources() public {
        // First call from BoundlessMarket (should skip verification)
        vm.prank(boundlessMarket);
        callback.handleProof(TEST_IMAGE_ID, TEST_JOURNAL, TEST_SEAL);
        assertEq(verifier.verifyCallCount(), 0);

        // Second call from random address (should verify)
        vm.prank(makeAddr("caller1"));
        callback.handleProof(TEST_IMAGE_ID, TEST_JOURNAL, TEST_SEAL);
        assertEq(verifier.verifyCallCount(), 1);

        // Third call from another random address (should verify)
        vm.prank(makeAddr("caller2"));
        callback.handleProof(TEST_IMAGE_ID, TEST_JOURNAL, TEST_SEAL);
        assertEq(verifier.verifyCallCount(), 2);

        // Fourth call from BoundlessMarket (should skip verification)
        vm.prank(boundlessMarket);
        callback.handleProof(TEST_IMAGE_ID, TEST_JOURNAL, TEST_SEAL);
        assertEq(verifier.verifyCallCount(), 2);
    }
}
