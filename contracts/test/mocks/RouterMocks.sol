// Copyright 2026 Boundless Foundation, Inc.
//
// Use of this source code is governed by the Business Source License
// as found in the LICENSE-BSL file.
// SPDX-License-Identifier: BUSL-1.1

pragma solidity ^0.8.26;

import {IERC165} from "@openzeppelin/contracts/utils/introspection/IERC165.sol";
import {IRiscZeroVerifier, Receipt} from "risc0/IRiscZeroVerifier.sol";

import {IBoundlessVerifier} from "../../src/router/interfaces/IBoundlessVerifier.sol";
import {IBoundlessAssessor} from "../../src/router/interfaces/IBoundlessAssessor.sol";
import {FulfillmentBatch} from "../../src/types/FulfillmentBatch.sol";

/// @notice Always-passing `IBoundlessVerifier`. Used by tests and benches that
///         want to isolate router/assessor cost from any real verifier work.
contract NullVerifier is IBoundlessVerifier, IERC165 {
    function verify(bytes calldata, bytes32) external pure {}

    function supportsInterface(bytes4 id) external pure returns (bool) {
        return id == type(IBoundlessVerifier).interfaceId || id == type(IERC165).interfaceId;
    }
}

/// @notice Always-passing `IBoundlessAssessor`. Used by tests that exercise
///         market state-machine logic without depending on a specific
///         assessor implementation, and by benches that isolate router
///         overhead from assessor work.
contract NullAssessor is IBoundlessAssessor, IERC165 {
    function verifyAssessor(FulfillmentBatch calldata, bytes32[] calldata) external pure {}

    function supportsInterface(bytes4 id) external pure returns (bool) {
        return id == type(IBoundlessAssessor).interfaceId || id == type(IERC165).interfaceId;
    }
}

/// @notice Always-passing `IRiscZeroVerifier`. Lets the R0 assessor adapter
///         run end-to-end in `forge test` without producing a real Groth16
///         proof — used in benches that measure the R0 adapter's wrapping
///         cost separately from the underlying STARK verify.
contract NullRiscZeroVerifier is IRiscZeroVerifier {
    function verify(bytes calldata, bytes32, bytes32) external view {}

    function verifyIntegrity(Receipt calldata) external view {}
}
