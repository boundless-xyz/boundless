// Copyright 2025 RISC Zero, Inc.
//
// Use of this source code is governed by the Business Source License
// as found in the LICENSE-BSL file.

pragma solidity ^0.8.20;

import {ReceiptClaim, ReceiptClaimLib} from "risc0/IRiscZeroVerifier.sol";
import {Test} from "forge-std/Test.sol";
import {Predicate, PredicateLibrary, PredicateType} from "../../src/types/Predicate.sol";

bytes32 constant IMAGE_ID = keccak256("ImageId for testing purposes");

contract PredicateTest is Test {
    using ReceiptClaimLib for ReceiptClaim;

    function testEvalDigestMatch() public pure {
        bytes32 hash = keccak256("test");
        Predicate memory predicate = PredicateLibrary.createDigestMatchPredicate(IMAGE_ID, hash);
        assertEq(
            uint8(predicate.predicateType), uint8(PredicateType.DigestMatch), "Predicate type should be DigestMatch"
        );

        bytes memory journal = "test";
        bytes32 journalDigest = keccak256(journal);

        bool result = predicate.eval(IMAGE_ID, journal, journalDigest);
        assertTrue(result, "Predicate evaluation should be true for matching digest");
    }

    function testEvalDigestMatchFail() public pure {
        bytes32 hash = keccak256("test");
        Predicate memory predicate = PredicateLibrary.createDigestMatchPredicate(IMAGE_ID, hash);
        assertEq(
            uint8(predicate.predicateType), uint8(PredicateType.DigestMatch), "Predicate type should be DigestMatch"
        );

        bytes memory journal = "different test";
        bytes32 journalDigest = keccak256(journal);

        bool result = predicate.eval(IMAGE_ID, journal, journalDigest);
        assertFalse(result, "Predicate evaluation should be false for non-matching digest");
    }

    function testEvalPrefixMatch() public pure {
        bytes memory prefix = "prefix";
        Predicate memory predicate = PredicateLibrary.createPrefixMatchPredicate(IMAGE_ID, prefix);
        bytes memory journal = "prefix and more";

        bool result = predicate.eval(IMAGE_ID, journal, keccak256(journal));
        assertTrue(result, "Predicate evaluation should be true for matching prefix");
    }

    function testEvalPrefixMatchFail() public pure {
        bytes memory prefix = "prefix";
        Predicate memory predicate = PredicateLibrary.createPrefixMatchPredicate(IMAGE_ID, prefix);
        bytes memory journal = "different prefix";

        bool result = predicate.eval(IMAGE_ID, journal, keccak256(journal));
        assertFalse(result, "Predicate evaluation should be false for non-matching prefix");
    }

    function testEvalClaimDigestMatch() public pure {
        bytes memory journal = "test";
        bytes32 journalDigest = keccak256(journal);
        bytes32 claimDigest = ReceiptClaimLib.ok(IMAGE_ID, journalDigest).digest();
        Predicate memory predicate = PredicateLibrary.createClaimDigestMatchPredicate(claimDigest);
        assertEq(
            uint8(predicate.predicateType),
            uint8(PredicateType.ClaimDigestMatch),
            "Predicate type should be ClaimDigestMatch"
        );

        bool result = predicate.eval(IMAGE_ID, journal, journalDigest);
        assertTrue(result, "Predicate evaluation should be true for matching digest");
    }

    function testEvalClaimDigestMatchFail() public pure {
        bytes memory journal = "test";
        bytes32 journalDigest = keccak256(journal);
        bytes32 claimDigest = ReceiptClaimLib.ok(IMAGE_ID, journalDigest).digest();
        Predicate memory predicate = PredicateLibrary.createClaimDigestMatchPredicate(claimDigest);
        assertEq(
            uint8(predicate.predicateType),
            uint8(PredicateType.ClaimDigestMatch),
            "Predicate type should be ClaimDigestMatch"
        );

        journal = "different test";
        journalDigest = keccak256(journal);

        bool result = predicate.eval(IMAGE_ID, journal, journalDigest);
        assertFalse(result, "Predicate evaluation should be false for non-matching digest");
    }
}
