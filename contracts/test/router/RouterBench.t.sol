// Copyright 2026 Boundless Foundation, Inc.
//
// Use of this source code is governed by the Business Source License
// as found in the LICENSE-BSL file.
// SPDX-License-Identifier: BUSL-1.1

pragma solidity ^0.8.26;

import {console2} from "forge-std/console2.sol";

import {BenchBase} from "./BenchBase.sol";
import {ProofRequest} from "../../src/types/ProofRequest.sol";
import {Fulfillment} from "../../src/types/Fulfillment.sol";
import {PredicateType} from "../../src/types/Predicate.sol";
import {SlimRequest} from "../../src/types/SlimRequest.sol";

/// @title RouterBench — measures `BoundlessRouter.verifyBatch` overhead.
///
/// @notice The bench is framed from the perspective of `BoundlessMarket` —
///         i.e. "what does the market pay per batch to drive the
///         verification engine, vs. the absolute minimum it could pay if
///         it hardcoded a single assessor adapter and skipped routing
///         entirely?"
///
///         Both Null adapters (verifier + assessor) return immediately, so
///         the numbers exclude adapter-internal work (predicate eval, STARK
///         verify, signature recover — benched separately in `AdapterBench`).
///         What remains is everything the *router architecture itself*
///         costs: per-class registry SLOADs, selector resolution,
///         signed-selector cross-check, mixed-class guard, and the two
///         categories of external calls the router emits per batch:
///           * 1 verifier STATICCALL per fill,
///           * 1 assessor STATICCALL per batch.
///
///         This is conceptually similar to the cost of dispatching through
///         `RiscZeroVerifierRouter` today — a selector → impl lookup plus a
///         STATICCALL — but generalized over the whole batch.
contract RouterBench is BenchBase {
    /// @notice B.1) Router framing cost (vs. minimal direct adapter call).
    ///         `direct-null` is the market's hypothetical lower bound:
    ///         one STATICCALL straight to a `NullAssessor` with no routing.
    ///         `router-null` is the production-shaped call: harness →
    ///         router → (N verifier hops + 1 assessor hop). The delta is
    ///         everything the router architecture adds on top — registry
    ///         SLOADs + dispatch logic + the per-fill verifier hops + the
    ///         router→assessor hop.
    ///
    ///         The delta is NOT "router-internal logic alone" — it includes
    ///         the cost of the per-fill verifier dispatch and the
    ///         router→assessor dispatch, both of which are intrinsic to the
    ///         pluggable architecture and cannot be subtracted from a
    ///         calling-market's perspective.
    function test_bench_routerFraming() external view {
        uint256[5] memory sizes = [uint256(1), 2, 5, 10, 50];

        console2.log("");
        console2.log("=== B.1) Router framing (NullVerifier + NullAssessor, DigestMatch fixtures) ===");
        for (uint256 k = 0; k < sizes.length; k++) {
            uint256 n = sizes[k];
            (ProofRequest[] memory r, Fulfillment[] memory f) = _buildBatch(n, PredicateType.DigestMatch);
            (SlimRequest[] memory s, bytes32[] memory rd) = _toSlimBatch(r);
            bytes memory nullSeal = _buildNullSeal();

            uint256 gDirect = directNull.measure(_makeBatch(s, f, proverAddr, nullSeal), rd);
            uint256 gRouter = routerHarness.measure(_makeBatch(s, f, proverAddr, nullSeal), rd);
            uint256 overhead = gRouter - gDirect;

            console2.log("  N=%d  direct-null=%d  router-null=%d", n, gDirect, gRouter);
            console2.log("           overhead=%d  overhead/fill=%d", overhead, overhead / n);
        }
    }

    /// @notice B.2) Cold vs warm.
    ///         Same call invoked twice in one tx. The first pays cold SLOADs
    ///         on `entries[]` / `classes[]` / `tombstoned[]`; the second is
    ///         warm. Delta isolates the cold-only portion of router overhead.
    function test_bench_coldVsWarm() external view {
        uint256[5] memory sizes = [uint256(1), 5, 10, 50, 100];

        console2.log("");
        console2.log("=== B.2) Cold vs warm router calls (NullAssessor, DigestMatch fixtures) ===");
        for (uint256 k = 0; k < sizes.length; k++) {
            uint256 n = sizes[k];
            (ProofRequest[] memory r, Fulfillment[] memory f) = _buildBatch(n, PredicateType.DigestMatch);
            (SlimRequest[] memory s, bytes32[] memory rd) = _toSlimBatch(r);
            bytes memory nullSeal = _buildNullSeal();
            (uint256 cold, uint256 warm) = multiCallHarness.measureColdWarm(_makeBatch(s, f, proverAddr, nullSeal), rd);
            console2.log("  N=%d  cold=%d  warm=%d", n, cold, warm);
            console2.log("           cold-only delta=%d", cold - warm);
        }
    }
}
