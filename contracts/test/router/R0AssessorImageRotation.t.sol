// Copyright 2026 Boundless Foundation, Inc.
//
// Use of this source code is governed by the Business Source License
// as found in the LICENSE-BSL file.
// SPDX-License-Identifier: BUSL-1.1

pragma solidity ^0.8.26;

import {ReceiptClaim, ReceiptClaimLib} from "risc0/IRiscZeroVerifier.sol";

import {BoundlessMarketTest, APP_JOURNAL, ASSESSOR_IMAGE_ID} from "../BoundlessMarket.t.sol";
import {BoundlessRouter} from "../../src/router/BoundlessRouter.sol";
import {R0BoundlessAssessorAdapter} from "../../src/router/adapters/R0BoundlessAssessorAdapter.sol";

import {SlimRequest} from "../../src/types/SlimRequest.sol";
import {Fulfillment} from "../../src/types/Fulfillment.sol";
import {FulfillmentBatch} from "../../src/types/FulfillmentBatch.sol";
import {FulfillmentDataType} from "../../src/types/FulfillmentData.sol";
import {ProofRequest} from "../../src/types/ProofRequest.sol";
import {LockRequest} from "../../src/types/LockRequest.sol";
import {IBoundlessMarket} from "../../src/IBoundlessMarket.sol";

import {Client} from "../clients/Client.sol";
import {TestUtils} from "../TestUtils.sol";
import {MerkleProofish} from "../../src/libraries/MerkleProofish.sol";

/// @title R0AssessorImageRotationTest — end-to-end rotation flow for the R0
///        assessor adapter.
///
/// @notice The deprecated-assessor concept has been replaced by router-level
///         tombstoning. The operational pattern is documented in
///         `R0BoundlessAssessorAdapter.sol:52-63`: deploy a new adapter
///         pinned to the new image id, `instantiate` it under the existing
///         assessor class at a fresh selector, run both in parallel, then
///         `removeEntry(oldSelector)` once brokers have migrated.
///
///         These tests model the rotation flow end-to-end:
///           - Both old and new selectors fulfill in parallel.
///           - After tombstoning the old selector, fulfillments via it
///             revert at the router while the same locked request still
///             settles via the new selector — brokers aren't stranded.
contract R0AssessorImageRotationTest is BoundlessMarketTest {
    using ReceiptClaimLib for ReceiptClaim;

    /// @notice Image id pinned by the NEW assessor adapter. Distinct from
    ///         `ASSESSOR_IMAGE_ID` (which the existing fixture's
    ///         `r0AssessorAdapter` is pinned to).
    bytes32 internal constant IMAGE_ID_NEW = bytes32(uint256(0x9999990100000000));
    /// @notice Router entry selector under the existing assessor class for
    ///         the NEW adapter. Brokers using the new image set this as the
    ///         first 4 bytes of `assessorSeal`.
    bytes4 internal constant ASSESSOR_R0_NEW_SEL = 0x00000025;

    R0BoundlessAssessorAdapter internal r0AssessorAdapterNew;

    function setUp() public override {
        super.setUp();
        // Deploy the second adapter and register it under the same assessor
        // class as the existing `r0AssessorAdapter`. After this, both
        // selectors point at production-grade adapters that share a `requiredAssessorClass`.
        vm.startPrank(ownerWallet.addr);
        r0AssessorAdapterNew = new R0BoundlessAssessorAdapter(setVerifier, IMAGE_ID_NEW);
        router.instantiate(ASSESSOR_R0_NEW_SEL, address(r0AssessorAdapterNew), ASSESSOR_CLASS_ID, 0);
        vm.stopPrank();
    }

    // ─── Rotation flow tests ────────────────────────────────────────────

    function test_rotation_bothSelectorsActiveInParallel() public {
        Client client = getClient(1);
        ProofRequest memory requestA = client.request(1);
        ProofRequest memory requestB = client.request(2);

        boundlessMarket.lockRequestWithSignature(
            requestA, client.sign(requestA), testProver.signLockRequest(LockRequest({request: requestA}))
        );
        boundlessMarket.lockRequestWithSignature(
            requestB, client.sign(requestB), testProver.signLockRequest(LockRequest({request: requestB}))
        );

        // Fulfill requestA via the OLD selector + image id (the production fixture's `r0AssessorAdapter`).
        FulfillmentBatch memory batchA = _buildBatchFor(requestA, APP_JOURNAL, ASSESSOR_IMAGE_ID, ASSESSOR_R0_SEL);
        boundlessMarket.fulfill(_asArray(batchA));
        expectRequestFulfilled(requestA.id);

        // Fulfill requestB via the NEW selector + image id (the rotationadapter we registered in this fixture's setUp).
        FulfillmentBatch memory batchB = _buildBatchFor(requestB, APP_JOURNAL, IMAGE_ID_NEW, ASSESSOR_R0_NEW_SEL);
        boundlessMarket.fulfill(_asArray(batchB));
        expectRequestFulfilled(requestB.id);
    }

    /// @dev Post-rotation lifecycle on a single locked request. Lock while
    ///      both adapters are live, tombstone the old selector mid-lock,
    ///      then exercise both broker behaviors against the same lock:
    ///      a stale broker (still on the old selector) gets a router
    ///      revert; a migrated broker (on the new selector) settles the
    ///      same request. The lock binds no assessor selector — that's
    ///      a per-fill broker choice — so the path to payment survives
    ///      the tombstone as long as the required assessor class has
    ///      any live entry.
    function test_rotation_postTombstone_oldRevertsAndLockedFulfillsViaNew() public {
        Client client = getClient(1);
        ProofRequest memory request = client.request(3);
        boundlessMarket.lockRequestWithSignature(
            request, client.sign(request), testProver.signLockRequest(LockRequest({request: request}))
        );

        // Governance tombstones the old selector while the lock is live.
        // The `requiredAssessorClass` still has a live entry (the new
        // adapter), so the class itself remains a valid fulfillment target.
        vm.prank(ownerWallet.addr);
        router.removeEntry(ASSESSOR_R0_SEL);

        // (1) Stale broker: still constructs the assessor seal against the
        // old selector. The router's `_entryOf(assessorSel)` hits the
        // tombstone branch and reverts; the market's `fulfill` bubbles
        // the revert without settling anything.
        FulfillmentBatch memory oldBatch = _buildBatchFor(request, APP_JOURNAL, ASSESSOR_IMAGE_ID, ASSESSOR_R0_SEL);
        vm.expectRevert(abi.encodeWithSelector(BoundlessRouter.EntryRemoved.selector, ASSESSOR_R0_SEL));
        boundlessMarket.fulfill(_asArray(oldBatch));
        expectRequestNotFulfilled(request.id);

        // (2) Migrated broker: produces the proof under the new image and
        // submits via the new selector. Same locked request, same prover,
        // same client — only the assessor seal differs. The request
        // settles, confirming the broker holding the lock is not
        // stranded by the tombstone.
        FulfillmentBatch memory newBatch = _buildBatchFor(request, APP_JOURNAL, IMAGE_ID_NEW, ASSESSOR_R0_NEW_SEL);
        boundlessMarket.fulfill(_asArray(newBatch));
        expectRequestFulfilled(request.id);
    }

    // ─── Helpers ─────────────────────────────────────────────────────────

    /// @dev Parameterized variant of `createFillsAndSubmitRootR0` from the
    ///      base fixture. Builds a single-fill `FulfillmentBatch` whose
    ///      assessor seal is `(assessorSelector || mockAssessorSeal)` and
    ///      whose set-builder root commits to a claim under `imageId`. The
    ///      two are parameters so this same helper can produce batches for
    ///      either the OLD or the NEW adapter.
    function _buildBatchFor(ProofRequest memory request, bytes memory journal, bytes32 imageId, bytes4 assessorSelector)
        internal
        returns (FulfillmentBatch memory batch)
    {
        ProofRequest[] memory requests = new ProofRequest[](1);
        requests[0] = request;
        bytes[] memory journals = new bytes[](1);
        journals[0] = journal;

        (Fulfillment[] memory fills, SlimRequest[] memory slim, bytes32[] memory requestDigests) =
            _buildFillsAndSlim(requests, journals, FulfillmentDataType.ImageIdAndJournal);
        bytes32 journalDigest = TestUtils.r0JournalDigest(slim, fills, requestDigests, testProverAddress);
        bytes32 assessorClaimDigest = ReceiptClaimLib.ok(imageId, journalDigest).digest();

        (bytes32 batchRoot, bytes32[][] memory tree) = TestUtils.mockSetBuilder(fills);
        bytes32 assessorLeaf = TestUtils.hashLeaf(assessorClaimDigest);
        bytes32 root = MerkleProofish._hashPair(batchRoot, assessorLeaf);
        TestUtils.fillInclusionProofs(setVerifier, fills, assessorLeaf, tree);
        submitRoot(root);

        batch = FulfillmentBatch({
            requests: slim,
            fills: fills,
            assessorSeal: abi.encodePacked(assessorSelector, TestUtils.mockAssessorSeal(setVerifier, batchRoot)),
            prover: testProverAddress
        });
    }
}
