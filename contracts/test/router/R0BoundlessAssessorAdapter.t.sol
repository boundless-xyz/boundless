// Copyright 2026 Boundless Foundation, Inc.
//
// Use of this source code is governed by the Business Source License
// as found in the LICENSE-BSL file.
// SPDX-License-Identifier: BUSL-1.1

pragma solidity ^0.8.26;

import {Test} from "forge-std/Test.sol";
import {IERC165} from "@openzeppelin/contracts/utils/introspection/IERC165.sol";
import {IRiscZeroVerifier} from "risc0/IRiscZeroVerifier.sol";

import {R0BoundlessAssessorAdapter} from "../../src/router/adapters/R0BoundlessAssessorAdapter.sol";
import {IBoundlessAssessor} from "../../src/router/interfaces/IBoundlessAssessor.sol";

import {SlimRequest} from "../../src/types/SlimRequest.sol";
import {Fulfillment} from "../../src/types/Fulfillment.sol";
import {FulfillmentBatch} from "../../src/types/FulfillmentBatch.sol";
import {FulfillmentDataType} from "../../src/types/FulfillmentData.sol";
import {Predicate, PredicateType} from "../../src/types/Predicate.sol";
import {Callback} from "../../src/types/Callback.sol";
import {RequestId} from "../../src/types/RequestId.sol";

import {NullRiscZeroVerifier} from "../mocks/RouterMocks.sol";
import {TestUtils} from "../TestUtils.sol";

/// @title R0BoundlessAssessorAdapterTest — unit tests for the R0 assessor adapter.
///
/// @notice Exercises the adapter independent of the router and market. The
///         adapter's job is to reconstruct the assessor guest's journal from
///         the slim payload + fills + caller-supplied digests, then forward
///         to the underlying `IRiscZeroVerifier.verify`. These tests:
///           1. Pin the input length + seal-format checks the adapter
///              applies before any cryptographic work.
///           2. Pin the journal-digest reconstruction by comparing against
///              `TestUtils.r0JournalDigest` (the same reference function
///              the market fixture uses to stand in for the guest).
///         Happy paths use a `NullRiscZeroVerifier` so we can isolate the
///         adapter's behavior; `vm.expectCall` asserts the exact innerSeal
///         and journalDigest passed downstream.
contract R0BoundlessAssessorAdapterTest is Test {
    R0BoundlessAssessorAdapter internal adapter;
    NullRiscZeroVerifier internal r0;

    bytes32 internal constant IMAGE_ID = bytes32(uint256(0xA55E550100000000));
    bytes4 internal constant ASSESSOR_SEL = 0xa00001a5;

    function setUp() public {
        r0 = new NullRiscZeroVerifier();
        adapter = new R0BoundlessAssessorAdapter(r0, IMAGE_ID);
    }

    // ─── Length checks ───────────────────────────────────────────────────

    function test_verifyAssessor_revertsOnFillsLengthMismatch() public {
        FulfillmentBatch memory batch = _baselineBatch(2);
        // Shrink fills to length 1 — requests stays length 2, mismatch.
        Fulfillment[] memory truncated = new Fulfillment[](1);
        truncated[0] = batch.fills[0];
        batch.fills = truncated;
        bytes32[] memory digests = _baselineDigests(2);

        vm.expectRevert(R0BoundlessAssessorAdapter.LengthMismatch.selector);
        adapter.verifyAssessor(batch, digests);
    }

    function test_verifyAssessor_revertsOnRequestDigestsLengthMismatch() public {
        FulfillmentBatch memory batch = _baselineBatch(2);
        bytes32[] memory digests = _baselineDigests(1); // size 1 ≠ n=2

        vm.expectRevert(R0BoundlessAssessorAdapter.LengthMismatch.selector);
        adapter.verifyAssessor(batch, digests);
    }

    // ─── Malformed seal ──────────────────────────────────────────────────

    function test_verifyAssessor_revertsOnEmptySeal() public {
        FulfillmentBatch memory batch = _baselineBatch(1);
        batch.assessorSeal = "";
        bytes32[] memory digests = _baselineDigests(1);

        vm.expectRevert(R0BoundlessAssessorAdapter.MalformedSeal.selector);
        adapter.verifyAssessor(batch, digests);
    }

    function test_verifyAssessor_revertsOnSubFourByteSeal() public {
        bytes32[] memory digests = _baselineDigests(1);
        // Lengths 1, 2, 3 — every length below the 4-byte selector prefix.
        for (uint256 len = 1; len <= 3; len++) {
            FulfillmentBatch memory batch = _baselineBatch(1);
            batch.assessorSeal = new bytes(len);
            vm.expectRevert(R0BoundlessAssessorAdapter.MalformedSeal.selector);
            adapter.verifyAssessor(batch, digests);
        }
    }

    function test_verifyAssessor_exactlyFourByteSealForwardsEmptyInnerSeal() public {
        // A 4-byte assessorSeal is the minimum that passes the length gate:
        // the selector prefix takes all 4, leaving innerSeal empty. The
        // adapter must NOT revert at the seal-length check here and must
        // forward an empty innerSeal to the underlying verifier.
        FulfillmentBatch memory batch = _baselineBatch(1);
        batch.assessorSeal = abi.encodePacked(ASSESSOR_SEL);
        bytes32[] memory digests = _baselineDigests(1);
        bytes32 expectedJournalDigest = TestUtils.r0JournalDigest(batch.requests, batch.fills, digests, batch.prover);

        vm.expectCall(
            address(r0), abi.encodeCall(IRiscZeroVerifier.verify, (bytes(""), IMAGE_ID, expectedJournalDigest))
        );
        adapter.verifyAssessor(batch, digests);
    }

    // ─── Inner seal stripping + journal forwarding ───────────────────────

    function test_verifyAssessor_forwardsInnerSealAndExpectedJournal() public {
        FulfillmentBatch memory batch = _baselineBatch(1);
        bytes memory innerSeal = hex"0102030405060708";
        batch.assessorSeal = abi.encodePacked(ASSESSOR_SEL, innerSeal);
        bytes32[] memory digests = _baselineDigests(1);
        bytes32 expectedJournalDigest = TestUtils.r0JournalDigest(batch.requests, batch.fills, digests, batch.prover);

        vm.expectCall(
            address(r0), abi.encodeCall(IRiscZeroVerifier.verify, (innerSeal, IMAGE_ID, expectedJournalDigest))
        );
        adapter.verifyAssessor(batch, digests);
    }

    // ─── Sparse callbacks / selectors ───────────────────────────────────

    function test_verifyAssessor_sparseArrays_noneOfThree() public {
        FulfillmentBatch memory batch = _baselineBatch(3);
        bytes32[] memory digests = _baselineDigests(3);
        // No callbacks, no selectors — both sparse arrays are length 0.
        _assertJournalRoundtrip(batch, digests);
    }

    function test_verifyAssessor_sparseArrays_someOfThree() public {
        FulfillmentBatch memory batch = _baselineBatch(3);
        // Non-contiguous indices 0 and 2 — exercises the AssessorCallback /
        // Selector `index` field as something other than the array position.
        batch.requests[0].callback = Callback({addr: address(0xC0FFEE), gasLimit: 12_000});
        batch.requests[2].callback = Callback({addr: address(0xBEEF), gasLimit: 23_000});
        batch.requests[0].selector = bytes4(0xa1a1a1a1);
        batch.requests[2].selector = bytes4(0xb2b2b2b2);
        bytes32[] memory digests = _baselineDigests(3);
        _assertJournalRoundtrip(batch, digests);
    }

    function test_verifyAssessor_sparseArrays_allOfThree() public {
        FulfillmentBatch memory batch = _baselineBatch(3);
        for (uint256 i = 0; i < 3; i++) {
            batch.requests[i].callback =
                Callback({addr: address(uint160(0xC0FFEE + i)), gasLimit: uint96(10_000 + i)});
            batch.requests[i].selector = bytes4(uint32(0xA0000001 + uint32(i)));
        }
        bytes32[] memory digests = _baselineDigests(3);
        _assertJournalRoundtrip(batch, digests);
    }

    // ─── Tamper detection ───────────────────────────────────────────────

    function test_verifyAssessor_tamperWithProverChangesJournal() public {
        FulfillmentBatch memory batch = _baselineBatch(2);
        bytes32[] memory digests = _baselineDigests(2);
        bytes32 baseline = TestUtils.r0JournalDigest(batch.requests, batch.fills, digests, batch.prover);

        batch.prover = address(0xDEADBEEF);
        bytes32 tampered = TestUtils.r0JournalDigest(batch.requests, batch.fills, digests, batch.prover);
        assertTrue(tampered != baseline, "prover change must shift journal digest");

        vm.expectCall(address(r0), abi.encodeCall(IRiscZeroVerifier.verify, (_innerSeal(), IMAGE_ID, tampered)));
        adapter.verifyAssessor(batch, digests);
    }

    function test_verifyAssessor_tamperWithSlimIdChangesJournal() public {
        FulfillmentBatch memory batch = _baselineBatch(2);
        bytes32[] memory digests = _baselineDigests(2);
        bytes32 baseline = TestUtils.r0JournalDigest(batch.requests, batch.fills, digests, batch.prover);

        batch.requests[0].id = RequestId.wrap(uint256(0xDEADBEEF));
        bytes32 tampered = TestUtils.r0JournalDigest(batch.requests, batch.fills, digests, batch.prover);
        assertTrue(tampered != baseline, "slim.id change must shift journal digest");

        vm.expectCall(address(r0), abi.encodeCall(IRiscZeroVerifier.verify, (_innerSeal(), IMAGE_ID, tampered)));
        adapter.verifyAssessor(batch, digests);
    }

    function test_verifyAssessor_tamperWithClaimDigestChangesJournal() public {
        FulfillmentBatch memory batch = _baselineBatch(2);
        bytes32[] memory digests = _baselineDigests(2);
        bytes32 baseline = TestUtils.r0JournalDigest(batch.requests, batch.fills, digests, batch.prover);

        batch.fills[0].claimDigest = bytes32(uint256(0xDEADBEEF));
        bytes32 tampered = TestUtils.r0JournalDigest(batch.requests, batch.fills, digests, batch.prover);
        assertTrue(tampered != baseline, "claimDigest change must shift journal digest");

        vm.expectCall(address(r0), abi.encodeCall(IRiscZeroVerifier.verify, (_innerSeal(), IMAGE_ID, tampered)));
        adapter.verifyAssessor(batch, digests);
    }

    function test_verifyAssessor_tamperWithFulfillmentDataChangesJournal() public {
        FulfillmentBatch memory batch = _baselineBatch(2);
        bytes32[] memory digests = _baselineDigests(2);
        bytes32 baseline = TestUtils.r0JournalDigest(batch.requests, batch.fills, digests, batch.prover);

        // Append a byte — fulfillmentDataDigest is keccak(uint8(type) || data),
        // so the leaf hash and therefore the journal both change.
        batch.fills[0].fulfillmentData = abi.encodePacked(batch.fills[0].fulfillmentData, hex"00");
        bytes32 tampered = TestUtils.r0JournalDigest(batch.requests, batch.fills, digests, batch.prover);
        assertTrue(tampered != baseline, "fulfillmentData change must shift journal digest");

        vm.expectCall(address(r0), abi.encodeCall(IRiscZeroVerifier.verify, (_innerSeal(), IMAGE_ID, tampered)));
        adapter.verifyAssessor(batch, digests);
    }

    // ─── Helpers ─────────────────────────────────────────────────────────

    /// @dev Compute the reference journal digest for `batch` + `digests` and
    ///      assert the adapter forwards it verbatim to R0.verify alongside
    ///      the expected innerSeal.
    function _assertJournalRoundtrip(FulfillmentBatch memory batch, bytes32[] memory digests) internal {
        bytes32 expectedJournalDigest = TestUtils.r0JournalDigest(batch.requests, batch.fills, digests, batch.prover);
        vm.expectCall(
            address(r0), abi.encodeCall(IRiscZeroVerifier.verify, (_innerSeal(), IMAGE_ID, expectedJournalDigest))
        );
        adapter.verifyAssessor(batch, digests);
    }

    /// @dev Build a baseline batch of `n` fills with deterministic, non-zero
    ///      slim/fill fields and an assessorSeal of `ASSESSOR_SEL || _innerSeal()`.
    function _baselineBatch(uint256 n) internal pure returns (FulfillmentBatch memory batch) {
        SlimRequest[] memory requests = new SlimRequest[](n);
        Fulfillment[] memory fills = new Fulfillment[](n);
        for (uint256 i = 0; i < n; i++) {
            requests[i] = SlimRequest({
                id: RequestId.wrap(uint256(0x1000) + i),
                predicate: Predicate({predicateType: PredicateType.ClaimDigestMatch, data: bytes("")}),
                callback: Callback({addr: address(0), gasLimit: 0}),
                selector: bytes4(0),
                imageUrlHash: bytes32(uint256(0xa000) + i),
                inputDigest: bytes32(uint256(0xb000) + i),
                offerDigest: bytes32(uint256(0xc000) + i)
            });
            fills[i] = Fulfillment({
                claimDigest: bytes32(uint256(0xd000) + i),
                fulfillmentDataType: FulfillmentDataType.None,
                fulfillmentData: bytes(""),
                seal: bytes("")
            });
        }
        batch = FulfillmentBatch({
            requests: requests,
            fills: fills,
            assessorSeal: abi.encodePacked(ASSESSOR_SEL, _innerSeal()),
            prover: address(0xBADCAFE)
        });
    }

    function _baselineDigests(uint256 n) internal pure returns (bytes32[] memory digests) {
        digests = new bytes32[](n);
        for (uint256 i = 0; i < n; i++) {
            digests[i] = bytes32(uint256(0xe000) + i);
        }
    }

    function _innerSeal() internal pure returns (bytes memory) {
        return hex"0102030405";
    }
}
