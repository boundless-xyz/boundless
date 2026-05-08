// Copyright 2026 Boundless Foundation, Inc.
//
// Use of this source code is governed by the Business Source License
// as found in the LICENSE-BSL file.
// SPDX-License-Identifier: BUSL-1.1

pragma solidity ^0.8.26;

import {IERC165} from "@openzeppelin/contracts/utils/introspection/IERC165.sol";
import {IRiscZeroVerifier, Receipt} from "risc0/IRiscZeroVerifier.sol";

import {IBoundlessVerifier} from "../interfaces/IBoundlessVerifier.sol";

/// @title R0BoundlessVerifierAdapter — `IBoundlessVerifier` adapter wrapping a
///        single specific `IRiscZeroVerifier` implementation.
///
/// @notice One adapter instance per BoundlessRouter selector entry, each bound at
///         deploy time to exactly one underlying verifier contract. Selector
///         dispatch happens *before* this contract is reached: BoundlessRouter
///         resolves the seal's first 4 bytes to this entry and calls us. We don't
///         read the selector ourselves; the underlying verifier re-checks it
///         against its own pinned value.
///
///         **Why one-per-selector and not one shared adapter** wrapping the
///         existing `RiscZeroVerifierRouter`: every selector reachable through
///         BoundlessRouter is then explicitly registered at the top level, with
///         an adapter address bound to exactly one underlying verifier. Reviewers
///         can audit BoundlessRouter's `entries` map alone — no transitive trust
///         of the upstream R0 router's selector set, and no second indirection at
///         verify time.
///
///         **Mapping from existing R0 selectors** (registered today in the
///         existing `RiscZeroVerifierRouter`):
///           * `Groth16V3_0      = 0x73c457ba`  → R0 Groth16 v3.0 verifier
///           * `SetVerifierV0_9  = 0x242f9d5b`  → `RiscZeroSetVerifier`
///           * `Blake3Groth16V0_1 = 0x62f049f6` → Blake3-Groth16 verifier
///
///         The Phase C deployment script instantiates one adapter per selector,
///         pinning it to the corresponding underlying verifier address (looked up
///         from the existing R0 router at deploy time), then registers each
///         adapter under the `R0_VERIFIER` class with its own selector entry.
contract R0BoundlessVerifierAdapter is IBoundlessVerifier, IERC165 {
    /// @notice The specific R0 verifier this adapter forwards to. Pinned at
    ///         deploy time to one underlying verifier (a Groth16 verifier, a
    ///         `RiscZeroSetVerifier`, etc.) — never the existing R0 router.
    ///         The underlying verifier reads `seal[0:4]` and reverts on selector
    ///         mismatch, so passing a seal whose selector doesn't match this
    ///         verifier's pinned value fails cleanly.
    IRiscZeroVerifier public immutable RISC_ZERO_VERIFIER;

    constructor(IRiscZeroVerifier riscZeroVerifier) {
        require(address(riscZeroVerifier) != address(0), "R0BoundlessVerifierAdapter: zero verifier");
        RISC_ZERO_VERIFIER = riscZeroVerifier;
    }

    /// @inheritdoc IBoundlessVerifier
    function verify(bytes calldata seal, bytes32 claimDigest) external view {
        RISC_ZERO_VERIFIER.verifyIntegrity(Receipt(seal, claimDigest));
    }

    /// @inheritdoc IERC165
    function supportsInterface(bytes4 interfaceId) external pure returns (bool) {
        return interfaceId == type(IBoundlessVerifier).interfaceId || interfaceId == type(IERC165).interfaceId;
    }
}
