// Copyright 2026 Boundless Foundation, Inc.
//
// Use of this source code is governed by the Business Source License
// as found in the LICENSE-BSL file.
// SPDX-License-Identifier: BUSL-1.1

pragma solidity ^0.8.13;

import {Test} from "forge-std/Test.sol";

import {
    Output,
    OutputLib,

    // Receipt needs to be renamed due to collision with type on the Test contract.
    Receipt as RiscZeroReceipt,
    ReceiptClaim,
    ReceiptClaimLib,
    SystemState,
    SystemStateLib,
    VerificationFailed
} from "risc0/IRiscZeroVerifier.sol";
import {ControlID} from "../src/blake3-groth16/ControlID.sol";
import {Blake3Groth16Verifier} from "../src/blake3-groth16/Blake3Groth16Verifier.sol";
import {TestReceipt} from "./receipts/Blake3Groth16TestReceipt.sol";

contract Blake3Groth16VerifierTest is Test {
    using OutputLib for Output;
    using ReceiptClaimLib for ReceiptClaim;
    using SystemStateLib for SystemState;

    RiscZeroReceipt internal receipt = RiscZeroReceipt(TestReceipt.SEAL, TestReceipt.CLAIM_DIGEST);

    Blake3Groth16Verifier internal verifier;

    function setUp() external {
        verifier = new Blake3Groth16Verifier(ControlID.CONTROL_ROOT, ControlID.BN254_CONTROL_ID);
    }

    function testConsistentSystemStateZeroDigest() external pure {
        require(
            ReceiptClaimLib.SYSTEM_STATE_ZERO_DIGEST
                == sha256(
                    abi.encodePacked(
                        SystemStateLib.TAG_DIGEST,
                        // down
                        bytes32(0),
                        // data
                        uint32(0),
                        // down.length
                        uint16(1) << 8
                    )
                )
        );
    }

    function testVerifyKnownGoodReceipt() external view {
        verifier.verifyIntegrity(receipt);
    }

    function expectVerificationFailure(bytes memory seal, ReceiptClaim memory claim) internal {
        bytes32 claimDigest = claim.digest();
        vm.expectRevert(VerificationFailed.selector);
        verifier.verifyIntegrity(RiscZeroReceipt(seal, claimDigest));
    }

    function testSelectorIsStable() external view {
        require(verifier.SELECTOR() == hex"62f049f6");
    }
}
