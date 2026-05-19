// Copyright 2026 Boundless Foundation, Inc.
//
// Use of this source code is governed by the Business Source License
// as found in the LICENSE-BSL file.
// SPDX-License-Identifier: BUSL-1.1

pragma solidity ^0.8.26;

import {BenchBase} from "../BenchBase.sol";
import {OnChainAssessor} from "../../../src/router/adapters/OnChainAssessor.sol";
import {ProofRequest} from "../../../src/types/ProofRequest.sol";
import {Fulfillment} from "../../../src/types/Fulfillment.sol";
import {FulfillmentDataType, FulfillmentDataImageIdAndJournal} from "../../../src/types/FulfillmentData.sol";
import {PredicateType} from "../../../src/types/Predicate.sol";
import {SlimRequest, SlimRequestLibrary} from "../../../src/types/SlimRequest.sol";

/// @title OnChainAssessorTest — unit tests for the native Solidity assessor.
///
/// @notice Covers the four soundness checks `OnChainAssessor` performs per
///         fulfillment batch: predicate evaluation, claim-digest binding to
///         the supplied (imageId, journal), prover-signature binding to the
///         supplied prover, and slim-payload digest reconstruction matching
///         the original `ProofRequest.eip712Digest()`.
///
///         Inherits from `BenchBase` for shared fixture builders and the
///         deployed router/adapter setup. The router is incidental here —
///         the harness calls the adapter directly.
contract OnChainAssessorTest is BenchBase {
    function test_slim_reconstructionMatchesFullDigest() external view {
        (ProofRequest[] memory rd,) = _buildBatch(3, PredicateType.DigestMatch);
        for (uint256 i = 0; i < rd.length; i++) {
            SlimRequest memory slim = _toSlim(rd[i]);
            assertEq(SlimRequestLibrary.reconstructRequestDigest(slim), rd[i].eip712Digest());
        }
    }

    function test_singleFill_digestMatch_passes() external view {
        (ProofRequest[] memory r, Fulfillment[] memory f) = _buildBatch(1, PredicateType.DigestMatch);
        (SlimRequest[] memory s, bytes32[] memory rd) = _toSlimBatch(r);
        bytes memory seal = _buildOnChainSeal(s, f);
        directOnChain.measure(_makeBatch(s, f, proverAddr, seal), rd);
    }

    function test_singleFill_claimDigestMatch_passes() external view {
        (ProofRequest[] memory r, Fulfillment[] memory f) = _buildBatch(1, PredicateType.ClaimDigestMatch);
        (SlimRequest[] memory s, bytes32[] memory rd) = _toSlimBatch(r);
        bytes memory seal = _buildOnChainSeal(s, f);
        directOnChain.measure(_makeBatch(s, f, proverAddr, seal), rd);
    }

    function test_predicateFailure_reverts() external {
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
        directOnChain.measure(_makeBatch(s, f, proverAddr, seal), rd);
    }

    function test_proverSignatureMismatch_reverts() external {
        (ProofRequest[] memory r, Fulfillment[] memory f) = _buildBatch(1, PredicateType.DigestMatch);
        (SlimRequest[] memory s, bytes32[] memory rd) = _toSlimBatch(r);
        // Signature is valid for `proverAddr`, but we pass a different address.
        // ECDSA.recover returns an unrelated address, so the assertion is just
        // that the mismatch is detected (match on error selector only).
        bytes memory seal = _buildOnChainSeal(s, f);
        vm.expectPartialRevert(OnChainAssessor.ProverSignatureMismatch.selector);
        directOnChain.measure(_makeBatch(s, f, address(0xDEAD), seal), rd);
    }

    function test_claimDigestMismatch_reverts() external {
        (ProofRequest[] memory r, Fulfillment[] memory f) = _buildBatch(1, PredicateType.DigestMatch);
        (SlimRequest[] memory s, bytes32[] memory rd) = _toSlimBatch(r);
        // Predicate eval passes (fulfillmentData intact), but the supplied
        // claim digest doesn't reconstruct from the journal — should revert.
        f[0].claimDigest = bytes32(uint256(f[0].claimDigest) ^ 1);
        bytes memory seal = _buildOnChainSeal(s, f);
        vm.expectRevert(abi.encodeWithSelector(OnChainAssessor.ClaimDigestMismatch.selector, uint256(0)));
        directOnChain.measure(_makeBatch(s, f, proverAddr, seal), rd);
    }
}
