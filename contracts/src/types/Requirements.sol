// Copyright 2025 RISC Zero, Inc.
//
// Use of this source code is governed by the Business Source License
// as found in the LICENSE-BSL file.
pragma solidity ^0.8.24;

import {Predicate, PredicateLibrary} from "./Predicate.sol";
import {Callback, CallbackLibrary} from "./Callback.sol";

using RequirementsLibrary for Requirements global;

struct Requirements {
    bytes32 imageId;
    Callback callback;
    Predicate predicate;
    bytes4 selector;
}

library RequirementsLibrary {
    string constant REQUIREMENTS_TYPE =
        "Requirements(bytes32 imageId,Callback callback,Predicate predicate,bytes4 selector)";
    bytes32 constant REQUIREMENTS_TYPEHASH =
        keccak256(abi.encodePacked(REQUIREMENTS_TYPE, CallbackLibrary.CALLBACK_TYPE, PredicateLibrary.PREDICATE_TYPE));

    // @notice Computes the EIP-712 digest of the requirements
    // @param requirements The requirements to digest
    // @return The EIP-712 digest of the requirements
    function eip712Digest(Requirements memory requirements) internal pure returns (bytes32) {
        return keccak256(
            abi.encode(
                REQUIREMENTS_TYPEHASH,
                requirements.imageId,
                CallbackLibrary.eip712Digest(requirements.callback),
                PredicateLibrary.eip712Digest(requirements.predicate),
                requirements.selector
            )
        );
    }
}
