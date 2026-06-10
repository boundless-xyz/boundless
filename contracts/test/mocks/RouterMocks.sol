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
import {IBoundlessJointVerifierAssessor} from "../../src/router/interfaces/IBoundlessJointVerifierAssessor.sol";
import {FulfillmentBatch} from "../../src/types/FulfillmentBatch.sol";
import {SlimRequest} from "../../src/types/SlimRequest.sol";
import {Fulfillment} from "../../src/types/Fulfillment.sol";

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

/// @notice Always-passing `IBoundlessJointVerifierAssessor`. Used by router
///         unit tests that exercise the joint-class dispatch path without
///         needing a real joint verifier.
contract NullJoint is IBoundlessJointVerifierAssessor, IERC165 {
    function verifyJoint(SlimRequest calldata, Fulfillment calldata, bytes32, address) external pure {}

    function supportsInterface(bytes4 id) external pure returns (bool) {
        return id == type(IBoundlessJointVerifierAssessor).interfaceId || id == type(IERC165).interfaceId;
    }
}

/// @notice `IBoundlessVerifier` that always reverts. Drives the router's
///         `VerifierFailed(i, sel)` catch path. Reverts with a known custom
///         error so tests can assert the catch wraps it.
contract RevertingVerifier is IBoundlessVerifier, IERC165 {
    error Boom();

    function verify(bytes calldata, bytes32) external pure {
        revert Boom();
    }

    function supportsInterface(bytes4 id) external pure returns (bool) {
        return id == type(IBoundlessVerifier).interfaceId || id == type(IERC165).interfaceId;
    }
}

/// @notice `IBoundlessJointVerifierAssessor` that always reverts. Drives the
///         per-fill catch path on the joint dispatch branch.
contract RevertingJoint is IBoundlessJointVerifierAssessor, IERC165 {
    error Boom();

    function verifyJoint(SlimRequest calldata, Fulfillment calldata, bytes32, address) external pure {
        revert Boom();
    }

    function supportsInterface(bytes4 id) external pure returns (bool) {
        return id == type(IBoundlessJointVerifierAssessor).interfaceId || id == type(IERC165).interfaceId;
    }
}

/// @notice `IBoundlessAssessor` that always reverts with a known custom
///         error payload. The router does NOT wrap assessor reverts in
///         try/catch — it forwards the staticcall and bubbles the revert
///         data verbatim. Used to assert byte-for-byte revert propagation.
contract RevertingAssessor is IBoundlessAssessor, IERC165 {
    error AssessorBoom(uint256 marker);

    function verifyAssessor(FulfillmentBatch calldata, bytes32[] calldata) external pure {
        revert AssessorBoom(0xDEADBEEF);
    }

    function supportsInterface(bytes4 id) external pure returns (bool) {
        return id == type(IBoundlessAssessor).interfaceId || id == type(IERC165).interfaceId;
    }
}

/// @notice `IBoundlessVerifier` that burns gas until forced out. The router
///         caps per-call gas via `staticcall{gas: e.gasLimit}`; this mock
///         lets tests assert that cap is enforced (caller observes
///         `VerifierFailed` after the catch).
contract GasHogVerifier is IBoundlessVerifier, IERC165 {
    function verify(bytes calldata, bytes32) external pure {
        // Burn gas without mutating state (the router calls via staticcall).
        // Keccak in a tight loop — runs until the gas cap exhausts.
        uint256 acc;
        while (true) {
            acc = uint256(keccak256(abi.encodePacked(acc)));
        }
    }

    function supportsInterface(bytes4 id) external pure returns (bool) {
        return id == type(IBoundlessVerifier).interfaceId || id == type(IERC165).interfaceId;
    }
}

/// @notice `IBoundlessVerifier` implementation that does NOT implement
///         ERC-165. `supportsInterface` doesn't exist, so the router's
///         `try/catch` on the ERC-165 probe falls into the failure branch
///         and rejects the impl at `instantiate`-time.
contract NonErc165Verifier is IBoundlessVerifier {
    function verify(bytes calldata, bytes32) external pure {}
}

/// @notice Implements `IERC165` but reports the wrong interface tag — claims
///         to be an assessor while exposing the verifier surface. The router
///         rejects this at `instantiate` time when the parent class's tag
///         doesn't match the impl's reported tag.
contract MisreportingErc165Verifier is IBoundlessVerifier, IERC165 {
    function verify(bytes calldata, bytes32) external pure {}

    function supportsInterface(bytes4 id) external pure returns (bool) {
        // Falsely reports the assessor tag, not the verifier tag.
        return id == type(IBoundlessAssessor).interfaceId || id == type(IERC165).interfaceId;
    }
}

/// @notice Declares `IERC165` but actively reverts in `supportsInterface`. The
///         router's `_supportsInterface` swallows the revert via try/catch and
///         treats it as `false`, so `instantiate` rejects this impl with
///         `Erc165CheckFailed`. Distinct from `NonErc165Verifier`, which
///         doesn't have the function at all.
contract RevertingErc165Verifier is IBoundlessVerifier, IERC165 {
    function verify(bytes calldata, bytes32) external pure {}

    function supportsInterface(bytes4) external pure returns (bool) {
        revert("erc165 boom");
    }
}

// Note: the assessor calldata-byte-equality assertion (see plan §B.9) is
// done via `vm.expectCall(target, exactCalldataBytes)` in the test, not a
// mock contract. The router calls the assessor via `staticcall`, so the
// callee cannot mutate state to record `msg.data`.

/// @notice `IBoundlessAssessor` that reverts with empty returndata. Used to
///         assert the router's calldata-forward bubbles a zero-length revert
///         as a zero-length revert (no wrapping in a custom error).
contract EmptyRevertAssessor is IBoundlessAssessor, IERC165 {
    function verifyAssessor(FulfillmentBatch calldata, bytes32[] calldata) external pure {
        assembly {
            revert(0, 0)
        }
    }

    function supportsInterface(bytes4 id) external pure returns (bool) {
        return id == type(IBoundlessAssessor).interfaceId || id == type(IERC165).interfaceId;
    }
}

/// @notice `IBoundlessAssessor` that burns gas in a tight keccak loop. The
///         router caps the assessor staticcall via `gas: e.gasLimit`; this
///         mock lets tests assert the cap is honored (outer revert bubbles
///         the empty OOG returndata).
contract GasHogAssessor is IBoundlessAssessor, IERC165 {
    function verifyAssessor(FulfillmentBatch calldata, bytes32[] calldata) external pure {
        uint256 acc;
        while (true) {
            acc = uint256(keccak256(abi.encodePacked(acc)));
        }
    }

    function supportsInterface(bytes4 id) external pure returns (bool) {
        return id == type(IBoundlessAssessor).interfaceId || id == type(IERC165).interfaceId;
    }
}
