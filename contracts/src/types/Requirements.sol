// Copyright (c) 2025 RISC Zero, Inc.
//
// All rights reserved.
pragma solidity ^0.8.20;

import {Predicate, PredicateLibrary} from "./Predicate.sol";

using RequirementsLibrary for Requirements global;

struct Requirements {
    bytes32 imageId;
    Predicate predicate;
    bytes4 selector;
}

library RequirementsLibrary {
    string constant REQUIREMENTS_TYPE = "Requirements(bytes32 imageId,Predicate predicate,bytes4 selector)";
    bytes32 constant REQUIREMENTS_TYPEHASH =
        keccak256(abi.encodePacked(REQUIREMENTS_TYPE, PredicateLibrary.PREDICATE_TYPE));

    function eip712Digest(Requirements memory requirements) internal pure returns (bytes32) {
        return keccak256(
            abi.encode(
                REQUIREMENTS_TYPEHASH,
                requirements.imageId,
                PredicateLibrary.eip712Digest(requirements.predicate),
                requirements.selector
            )
        );
    }
}
