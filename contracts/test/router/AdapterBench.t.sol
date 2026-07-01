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

/// @title AdapterBench — measures individual assessor adapters.
///
/// @notice Benchmarks the assessor adapters via direct call (no router). Each
///         row reports the adapter's own gas: predicate evaluation,
///         signature/STARK verification, claim-digest binding, etc. The market
///         binding check and the router dispatch are out of scope here.
contract AdapterBench is BenchBase {
    /// @notice A) Compare adapters apples-to-apples by direct call. Uses the
    ///         order-generator-sized 16-byte journal (~80% of Base traffic).
    function test_bench_adapters() external view {
        uint256[6] memory sizes = [uint256(1), 2, 5, 10, 50, 100];

        console2.log("");
        console2.log("=== Adapter comparison: DigestMatch, 16-byte journal, per-fill gas ===");
        console2.log(
            "    R0 column excludes the underlying Groth16 verify; add %d gas/batch for the real cost if not using set builder.",
            R0_GROTH16_VERIFY_GAS
        );
        for (uint256 k = 0; k < sizes.length; k++) {
            uint256 n = sizes[k];
            (ProofRequest[] memory r, Fulfillment[] memory f) = _buildBatch(n, PredicateType.DigestMatch);
            (SlimRequest[] memory s, bytes32[] memory rd) = _toSlimBatch(r);
            bytes memory onChainSeal = _buildOnChainSeal(s, f);
            bytes memory r0Seal = _buildR0Seal();

            uint256 gOnChain = directOnChain.measure(_makeBatch(s, f, proverAddr, onChainSeal), rd);
            uint256 gR0 = directR0.measure(_makeBatch(s, f, proverAddr, r0Seal), rd);

            console2.log("  N=%d  onChain/fill=%d  R0/fill=%d", n, gOnChain / n, gR0 / n);
        }

        console2.log("");
        console2.log("=== Adapter comparison: ClaimDigestMatch, per-fill gas ===");
        for (uint256 k = 0; k < sizes.length; k++) {
            uint256 n = sizes[k];
            (ProofRequest[] memory r, Fulfillment[] memory f) = _buildBatch(n, PredicateType.ClaimDigestMatch);
            (SlimRequest[] memory s, bytes32[] memory rd) = _toSlimBatch(r);
            bytes memory onChainSeal = _buildOnChainSeal(s, f);
            bytes memory r0Seal = _buildR0Seal();

            uint256 gOnChain = directOnChain.measure(_makeBatch(s, f, proverAddr, onChainSeal), rd);
            uint256 gR0 = directR0.measure(_makeBatch(s, f, proverAddr, r0Seal), rd);

            console2.log("  N=%d  onChain/fill=%d  R0/fill=%d", n, gOnChain / n, gR0 / n);
        }
    }

    /// @notice Show how journal size affects per-fill cost.
    function test_bench_journalSize() external view {
        uint256[3] memory journalSizes = [uint256(16), 128, 512];
        uint256 n = 10;

        console2.log("");
        console2.log("=== Journal-size sweep at N=10, DigestMatch, per-fill gas ===");
        for (uint256 k = 0; k < journalSizes.length; k++) {
            uint256 jbytes = journalSizes[k];
            (ProofRequest[] memory r, Fulfillment[] memory f) = _buildBatch(n, PredicateType.DigestMatch, jbytes);
            (SlimRequest[] memory s, bytes32[] memory rd) = _toSlimBatch(r);
            bytes memory onChainSeal = _buildOnChainSeal(s, f);
            bytes memory r0Seal = _buildR0Seal();

            uint256 gOnChain = directOnChain.measure(_makeBatch(s, f, proverAddr, onChainSeal), rd);
            uint256 gR0 = directR0.measure(_makeBatch(s, f, proverAddr, r0Seal), rd);

            console2.log("  journal=%d bytes  onChain/fill=%d  R0/fill=%d", jbytes, gOnChain / n, gR0 / n);
        }

        console2.log("");
        console2.log("=== Journal-size sweep at N=10, ClaimDigestMatch, per-fill gas ===");
        for (uint256 k = 0; k < journalSizes.length; k++) {
            uint256 jbytes = journalSizes[k];
            (ProofRequest[] memory r, Fulfillment[] memory f) = _buildBatch(n, PredicateType.ClaimDigestMatch, jbytes);
            (SlimRequest[] memory s, bytes32[] memory rd) = _toSlimBatch(r);
            bytes memory onChainSeal = _buildOnChainSeal(s, f);
            bytes memory r0Seal = _buildR0Seal();

            uint256 gOnChain = directOnChain.measure(_makeBatch(s, f, proverAddr, onChainSeal), rd);
            uint256 gR0 = directR0.measure(_makeBatch(s, f, proverAddr, r0Seal), rd);

            console2.log("  journal=%d bytes  onChain/fill=%d  R0/fill=%d", jbytes, gOnChain / n, gR0 / n);
        }
    }
}
