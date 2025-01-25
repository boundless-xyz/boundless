// Copyright (c) 2025 RISC Zero, Inc.
//
// All rights reserved.

pragma solidity ^0.8.20;

import "forge-std/Test.sol";
import {Predicate, PredicateLibrary, PredicateType} from "../../src/types/Predicate.sol";

contract PredicateTest is Test {
    function testEvalDigestMatch() public pure {
        bytes32 hash = keccak256("test");
        Predicate memory predicate = PredicateLibrary.createDigestMatchPredicate(hash);
        assertEq(
            uint8(predicate.predicateType), uint8(PredicateType.DigestMatch), "Predicate type should be DigestMatch"
        );

        bytes memory journal = "test";
        bytes32 journalDigest = keccak256(journal);

        bool result = predicate.eval(journal, journalDigest);
        assertTrue(result, "Predicate evaluation should be true for matching digest");
    }

    function testEvalDigestMatchFail() public pure {
        bytes32 hash = keccak256("test");
        Predicate memory predicate = PredicateLibrary.createDigestMatchPredicate(hash);
        assertEq(
            uint8(predicate.predicateType), uint8(PredicateType.DigestMatch), "Predicate type should be DigestMatch"
        );

        bytes memory journal = "different test";
        bytes32 journalDigest = keccak256(journal);

        bool result = predicate.eval(journal, journalDigest);
        assertFalse(result, "Predicate evaluation should be false for non-matching digest");
    }

    function testEvalPrefixMatch() public pure {
        bytes memory prefix = "prefix";
        Predicate memory predicate = PredicateLibrary.createPrefixMatchPredicate(prefix);
        bytes memory journal = "prefix and more";

        bool result = predicate.eval(journal, keccak256(journal));
        assertTrue(result, "Predicate evaluation should be true for matching prefix");
    }

    function testEvalPrefixMatchFail() public pure {
        bytes memory prefix = "prefix";
        Predicate memory predicate = PredicateLibrary.createPrefixMatchPredicate(prefix);
        bytes memory journal = "different prefix";

        bool result = predicate.eval(journal, keccak256(journal));
        assertFalse(result, "Predicate evaluation should be false for non-matching prefix");
    }
}
