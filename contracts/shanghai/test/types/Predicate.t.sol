// Copyright 2026 Boundless Foundation, Inc.
//
// Use of this source code is governed by the Business Source License
// as found in the LICENSE-BSL file.

pragma solidity ^0.8.26;

import {ReceiptClaim, ReceiptClaimLib} from "risc0/IRiscZeroVerifier.sol";
import {Test} from "forge-std/Test.sol";
import {Predicate, PredicateLibrary, PredicateType} from "../../src/types/Predicate.sol";

bytes32 constant IMAGE_ID = keccak256("ImageId for testing purposes");

contract PredicateTest is Test {
    using ReceiptClaimLib for ReceiptClaim;

    function testEvalDigestMatch() public pure {
        bytes32 hash = sha256(abi.encode("test"));
        Predicate memory predicate = PredicateLibrary.createDigestMatchPredicate(IMAGE_ID, hash);
        assertEq(
            uint8(predicate.predicateType), uint8(PredicateType.DigestMatch), "Predicate type should be DigestMatch"
        );

        bytes memory journal = "test";

        bool result = predicate.eval(IMAGE_ID, journal);
        assertTrue(result, "Predicate evaluation should be true for matching digest");
    }

    function testEvalDigestMatchFail() public pure {
        bytes32 hash = sha256(abi.encode("test"));
        Predicate memory predicate = PredicateLibrary.createDigestMatchPredicate(IMAGE_ID, hash);
        assertEq(
            uint8(predicate.predicateType), uint8(PredicateType.DigestMatch), "Predicate type should be DigestMatch"
        );

        bytes memory journal = "different test";

        bool result = predicate.eval(IMAGE_ID, journal);
        assertFalse(result, "Predicate evaluation should be false for non-matching digest");
    }

    function testEvalPrefixMatch() public pure {
        bytes memory prefix = "prefix";
        Predicate memory predicate = PredicateLibrary.createPrefixMatchPredicate(IMAGE_ID, prefix);
        bytes memory journal = "prefix and more";

        bool result = predicate.eval(IMAGE_ID, journal);
        assertTrue(result, "Predicate evaluation should be true for matching prefix");
    }

    function testEvalPrefixMatchFail() public pure {
        bytes memory prefix = "prefix";
        Predicate memory predicate = PredicateLibrary.createPrefixMatchPredicate(IMAGE_ID, prefix);
        bytes memory journal = "different prefix";

        bool result = predicate.eval(IMAGE_ID, journal);
        assertFalse(result, "Predicate evaluation should be false for non-matching prefix");
    }

    function testEvalClaimDigestMatch() public pure {
        bytes memory journal = "test";
        bytes32 journalDigest = sha256(abi.encode(journal));
        bytes32 claimDigest = ReceiptClaimLib.ok(IMAGE_ID, journalDigest).digest();
        Predicate memory predicate = PredicateLibrary.createClaimDigestMatchPredicate(claimDigest);
        assertEq(
            uint8(predicate.predicateType),
            uint8(PredicateType.ClaimDigestMatch),
            "Predicate type should be ClaimDigestMatch"
        );

        bool result = predicate.eval(claimDigest);
        assertTrue(result, "Predicate evaluation should be true for matching digest");
    }

    function testEvalClaimDigestMatchFail() public pure {
        bytes memory journal = "test";
        bytes32 journalDigest = sha256(abi.encode(journal));
        bytes32 claimDigest = ReceiptClaimLib.ok(IMAGE_ID, journalDigest).digest();
        Predicate memory predicate = PredicateLibrary.createClaimDigestMatchPredicate(claimDigest);
        assertEq(
            uint8(predicate.predicateType),
            uint8(PredicateType.ClaimDigestMatch),
            "Predicate type should be ClaimDigestMatch"
        );

        journal = "different test";
        journalDigest = sha256(abi.encode(journal));
        claimDigest = ReceiptClaimLib.ok(IMAGE_ID, journalDigest).digest();

        bool result = predicate.eval(claimDigest);
        assertFalse(result, "Predicate evaluation should be false for non-matching digest");
    }
}
