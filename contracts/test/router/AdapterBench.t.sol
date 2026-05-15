// Copyright 2026 Boundless Foundation, Inc.
//
// Use of this source code is governed by the Business Source License
// as found in the LICENSE-BSL file.
// SPDX-License-Identifier: BUSL-1.1

pragma solidity ^0.8.26;

import {console2} from "forge-std/console2.sol";

import {BenchBase} from "./BenchBase.sol";
import {OnChainAssessor} from "../../src/router/adapters/OnChainAssessor.sol";
import {ProofRequest} from "../../src/types/ProofRequest.sol";
import {Fulfillment} from "../../src/types/Fulfillment.sol";
import {FulfillmentDataType, FulfillmentDataImageIdAndJournal} from "../../src/types/FulfillmentData.sol";
import {PredicateType} from "../../src/types/Predicate.sol";
import {SlimRequest, SlimRequestLibrary} from "../../src/types/SlimRequest.sol";

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
        uint256[5] memory sizes = [uint256(1), 5, 10, 50, 100];

        console2.log("");
        console2.log("=== Adapter comparison: DigestMatch, 16-byte journal, per-fill gas ===");
        console2.log(
            "    R0 column excludes the underlying Groth16 verify; add %d gas/batch for the real cost.",
            R0_GROTH16_VERIFY_GAS
        );
        for (uint256 k = 0; k < sizes.length; k++) {
            uint256 n = sizes[k];
            (ProofRequest[] memory r, Fulfillment[] memory f) = _buildBatch(n, PredicateType.DigestMatch);
            (SlimRequest[] memory s, bytes32[] memory rd) = _toSlimBatch(r);
            bytes memory onChainSeal = _buildOnChainSeal(s, f);
            bytes memory r0Seal = _buildR0Seal();

            uint256 gOnChain = directOnChain.measure(s, f, rd, proverAddr, onChainSeal);
            uint256 gR0 = directR0.measure(s, f, rd, proverAddr, r0Seal);

            console2.log("  N=%d  onChain/fill=%d  R0/fill=%d", n, gOnChain / n, gR0 / n);
        }

        console2.log("");
        console2.log("=== Adapter comparison: ClaimDigestMatch, 16-byte journal, per-fill gas ===");
        for (uint256 k = 0; k < sizes.length; k++) {
            uint256 n = sizes[k];
            (ProofRequest[] memory r, Fulfillment[] memory f) = _buildBatch(n, PredicateType.ClaimDigestMatch);
            (SlimRequest[] memory s, bytes32[] memory rd) = _toSlimBatch(r);
            bytes memory onChainSeal = _buildOnChainSeal(s, f);
            bytes memory r0Seal = _buildR0Seal();

            uint256 gOnChain = directOnChain.measure(s, f, rd, proverAddr, onChainSeal);
            uint256 gR0 = directR0.measure(s, f, rd, proverAddr, r0Seal);

            console2.log("  N=%d  onChain/fill=%d  R0/fill=%d", n, gOnChain / n, gR0 / n);
        }
    }

    /// @notice Show how journal size affects per-fill cost. The on-chain
    ///         DigestMatch path does `sha256(abi.encode(journal))` twice per
    ///         fill (once for predicate eval, once for claim-digest binding),
    ///         so its cost grows linearly with journal length. R0 hashes the
    ///         journal once when computing `fulfillmentDataDigest`. The
    ///         ClaimDigestMatch path doesn't touch the journal at all.
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

            uint256 gOnChain = directOnChain.measure(s, f, rd, proverAddr, onChainSeal);
            uint256 gR0 = directR0.measure(s, f, rd, proverAddr, r0Seal);

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

            uint256 gOnChain = directOnChain.measure(s, f, rd, proverAddr, onChainSeal);
            uint256 gR0 = directR0.measure(s, f, rd, proverAddr, r0Seal);

            console2.log("  journal=%d bytes  onChain/fill=%d  R0/fill=%d", jbytes, gOnChain / n, gR0 / n);
        }
    }

    // ─── Sanity ───────────────────────────────────────────────────────────

    function test_slim_reconstructionMatchesFullDigest() external view {
        (ProofRequest[] memory rd,) = _buildBatch(3, PredicateType.DigestMatch);
        for (uint256 i = 0; i < rd.length; i++) {
            SlimRequest memory slim = _toSlim(rd[i]);
            assertEq(SlimRequestLibrary.reconstructRequestDigest(slim), rd[i].eip712Digest());
        }
    }

    function test_onChain_singleFill_passes() external {
        (ProofRequest[] memory r, Fulfillment[] memory f) = _buildBatch(1, PredicateType.DigestMatch);
        (SlimRequest[] memory s, bytes32[] memory rd) = _toSlimBatch(r);
        bytes memory seal = _buildOnChainSeal(s, f);
        directOnChain.measure(s, f, rd, proverAddr, seal);
    }

    function test_r0_singleFill_passes() external {
        (ProofRequest[] memory r, Fulfillment[] memory f) = _buildBatch(1, PredicateType.DigestMatch);
        (SlimRequest[] memory s, bytes32[] memory rd) = _toSlimBatch(r);
        bytes memory seal = _buildR0Seal();
        directR0.measure(s, f, rd, proverAddr, seal);
    }

    function test_predicateFailureReverts() external {
        (ProofRequest[] memory r, Fulfillment[] memory f) = _buildBatch(1, PredicateType.DigestMatch);
        (SlimRequest[] memory s, bytes32[] memory rd) = _toSlimBatch(r);
        // Tamper with fulfillment journal — predicate eval should fail before
        // the signature check, so the seal contents don't matter.
        bytes memory wrongJournal = bytes("not-the-journal");
        (bytes32 imageId,) = _imageAndJournal(0);
        f[0].fulfillmentData =
            abi.encode(FulfillmentDataImageIdAndJournal({imageId: imageId, journal: wrongJournal}));
        bytes memory seal = _buildOnChainSeal(s, f);

        vm.expectRevert(abi.encodeWithSelector(OnChainAssessor.PredicateFailed.selector, uint256(0)));
        directOnChain.measure(s, f, rd, proverAddr, seal);
    }

    function test_proverSignatureMismatchReverts() external {
        (ProofRequest[] memory r, Fulfillment[] memory f) = _buildBatch(1, PredicateType.DigestMatch);
        (SlimRequest[] memory s, bytes32[] memory rd) = _toSlimBatch(r);
        bytes memory seal = _buildOnChainSeal(s, f);
        vm.expectPartialRevert(OnChainAssessor.ProverSignatureMismatch.selector);
        directOnChain.measure(s, f, rd, address(0xDEAD), seal);
    }

    function test_claimDigestMismatchReverts() external {
        (ProofRequest[] memory r, Fulfillment[] memory f) = _buildBatch(1, PredicateType.DigestMatch);
        (SlimRequest[] memory s, bytes32[] memory rd) = _toSlimBatch(r);
        // Predicate eval passes (fulfillmentData intact), but claim digest is broken.
        f[0].claimDigest = bytes32(uint256(f[0].claimDigest) ^ 1);
        bytes memory seal = _buildOnChainSeal(s, f);
        vm.expectRevert(abi.encodeWithSelector(OnChainAssessor.ClaimDigestMismatch.selector, uint256(0)));
        directOnChain.measure(s, f, rd, proverAddr, seal);
    }
}
