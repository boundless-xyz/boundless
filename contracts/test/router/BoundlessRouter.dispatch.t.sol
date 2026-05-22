// Copyright 2026 Boundless Foundation, Inc.
//
// Use of this source code is governed by the Business Source License
// as found in the LICENSE-BSL file.
// SPDX-License-Identifier: BUSL-1.1

pragma solidity ^0.8.26;

import {RouterTestBase} from "./RouterTestBase.sol";
import {BoundlessRouter} from "../../src/router/BoundlessRouter.sol";
import {IBoundlessVerifier} from "../../src/router/interfaces/IBoundlessVerifier.sol";
import {IBoundlessJointVerifierAssessor} from "../../src/router/interfaces/IBoundlessJointVerifierAssessor.sol";
import {IBoundlessAssessor} from "../../src/router/interfaces/IBoundlessAssessor.sol";

import {SlimRequest} from "../../src/types/SlimRequest.sol";
import {Fulfillment} from "../../src/types/Fulfillment.sol";
import {FulfillmentBatch} from "../../src/types/FulfillmentBatch.sol";
import {FulfillmentDataType} from "../../src/types/FulfillmentData.sol";
import {Predicate, PredicateType} from "../../src/types/Predicate.sol";
import {Callback} from "../../src/types/Callback.sol";
import {RequestId} from "../../src/types/RequestId.sol";

import {
    NullVerifier,
    NullAssessor,
    NullJoint,
    RevertingVerifier,
    RevertingJoint,
    RevertingAssessor,
    EmptyRevertAssessor,
    GasHogAssessor,
    GasHogVerifier
} from "../mocks/RouterMocks.sol";

import {IBoundlessRouter} from "../../src/router/interfaces/IBoundlessRouter.sol";

/// @title BoundlessRouterDispatchTest — Section B of the router test plan.
///
/// @notice Covers `verifyBatch` end-to-end: length/shape guards, first-seal
///         resolution, per-fill verifier-class and joint-class dispatch,
///         mixed-class detection, assessor dispatch, dispatch ordering, and
///         signed-selector resolution. Assessor calldata-forwarding lives in
///         a later sub-section.
contract BoundlessRouterDispatchTest is RouterTestBase {
    bytes4 internal constant V_CLASS = 0x00000010;
    bytes4 internal constant V_ENTRY = 0x00000011;
    bytes4 internal constant V_ENTRY_2 = 0x00000012;
    bytes4 internal constant A_CLASS = 0x00000020;
    bytes4 internal constant A_ENTRY = 0x00000021;
    bytes4 internal constant J_CLASS = 0x00000030;
    bytes4 internal constant J_ENTRY = 0x00000031;
    bytes4 internal constant A_CLASS_2 = 0x00000022;
    bytes4 internal constant A_ENTRY_2 = 0x00000023;

    address internal verifierImpl;
    address internal assessorImpl;
    address internal jointImpl;

    function setUp() public override {
        super.setUp();
        verifierImpl = address(new NullVerifier());
        assessorImpl = address(new NullAssessor());
        jointImpl = address(new NullJoint());
    }

    // ─── Helpers ──────────────────────────────────────────────────────────

    /// @dev Stand-up "verifier class V → assessor class A → impls pinned at
    ///      entries V_ENTRY/A_ENTRY". The minimal happy-path fixture.
    function _setupVerifierEcosystem() internal {
        _addAssessorClass(A_CLASS, false);
        _addVerifierClass(V_CLASS, A_CLASS, false, false);
        _instantiateAsAdmin(A_ENTRY, assessorImpl, A_CLASS);
        _instantiateAsAdmin(V_ENTRY, verifierImpl, V_CLASS);
    }

    function _setupJointEcosystem() internal {
        _addJointClass(J_CLASS, false);
        _instantiateAsAdmin(J_ENTRY, jointImpl, J_CLASS);
    }

    /// @dev Build a batch with `n` fills under V_ENTRY, signed-selector == V_ENTRY,
    ///      assessor seal pointing at A_ENTRY. Tests that need other shapes mutate
    ///      after calling this.
    function _verifierBatch(uint256 n) internal pure returns (FulfillmentBatch memory batch) {
        SlimRequest[] memory requests = new SlimRequest[](n);
        Fulfillment[] memory fills = new Fulfillment[](n);
        for (uint256 i = 0; i < n; i++) {
            requests[i] = SlimRequest({
                id: RequestId.wrap(0),
                predicate: Predicate({predicateType: PredicateType.ClaimDigestMatch, data: bytes("")}),
                callback: Callback({addr: address(0), gasLimit: 0}),
                selector: V_ENTRY,
                imageUrlHash: bytes32(0),
                inputDigest: bytes32(0),
                offerDigest: bytes32(0)
            });
            fills[i] = Fulfillment({
                claimDigest: bytes32(uint256(0xC1A1) + i),
                fulfillmentDataType: FulfillmentDataType.None,
                fulfillmentData: bytes(""),
                seal: abi.encodePacked(V_ENTRY, hex"deadbeef")
            });
        }
        batch = FulfillmentBatch({
            requests: requests,
            fills: fills,
            assessorSeal: abi.encodePacked(A_ENTRY, hex"cafe"),
            prover: address(0xBEEF)
        });
    }

    function _jointBatch(uint256 n) internal pure returns (FulfillmentBatch memory batch) {
        // Joint batches differ from verifier batches only in the per-fill
        // entry selector and the absence of an assessor seal.
        batch = _verifierBatch(n);
        for (uint256 i = 0; i < n; i++) {
            batch.requests[i].selector = J_ENTRY;
            batch.fills[i].seal = abi.encodePacked(J_ENTRY, hex"deadbeef");
        }
        batch.assessorSeal = bytes("");
    }

    // ─── B.1 Length / shape guards ────────────────────────────────────────

    function test_verifyBatch_revertsOnEmptyBatch() public {
        _setupVerifierEcosystem();
        FulfillmentBatch memory batch = _verifierBatch(0);
        bytes32[] memory digests = new bytes32[](0);
        vm.expectRevert(BoundlessRouter.EmptyBatch.selector);
        router.verifyBatch(batch, digests);
    }

    function test_verifyBatch_revertsOnRequestsLengthMismatch() public {
        _setupVerifierEcosystem();
        FulfillmentBatch memory batch = _verifierBatch(2);
        // Drop one request — fills (2) vs requests (1) mismatch.
        SlimRequest[] memory shorter = new SlimRequest[](1);
        shorter[0] = batch.requests[0];
        batch.requests = shorter;
        bytes32[] memory digests = new bytes32[](2);
        vm.expectRevert(BoundlessRouter.LengthMismatch.selector);
        router.verifyBatch(batch, digests);
    }

    function test_verifyBatch_revertsOnRequestDigestsLengthMismatch() public {
        _setupVerifierEcosystem();
        FulfillmentBatch memory batch = _verifierBatch(2);
        bytes32[] memory digests = new bytes32[](1); // wrong length
        vm.expectRevert(BoundlessRouter.LengthMismatch.selector);
        router.verifyBatch(batch, digests);
    }

    // ─── B.2 First-seal resolution ────────────────────────────────────────

    function test_verifyBatch_revertsOnFirstSealMalformed() public {
        _setupVerifierEcosystem();
        FulfillmentBatch memory batch = _verifierBatch(1);
        batch.fills[0].seal = hex"010203"; // 3 bytes — short of a selector.
        bytes32[] memory digests = new bytes32[](1);
        vm.expectRevert(BoundlessRouter.MalformedSeal.selector);
        router.verifyBatch(batch, digests);
    }

    function test_verifyBatch_revertsOnFirstSealSelectorIsZero() public {
        _setupVerifierEcosystem();
        FulfillmentBatch memory batch = _verifierBatch(1);
        batch.fills[0].seal = abi.encodePacked(bytes4(0), hex"deadbeef");
        bytes32[] memory digests = new bytes32[](1);
        vm.expectRevert(BoundlessRouter.ZeroSelectorReserved.selector);
        router.verifyBatch(batch, digests);
    }

    function test_verifyBatch_revertsOnFirstSealSelectorUnknown() public {
        _setupVerifierEcosystem();
        FulfillmentBatch memory batch = _verifierBatch(1);
        bytes4 ghost = 0x0000_00FF;
        batch.fills[0].seal = abi.encodePacked(ghost, hex"deadbeef");
        bytes32[] memory digests = new bytes32[](1);
        vm.expectRevert(abi.encodeWithSelector(BoundlessRouter.EntryUnknown.selector, ghost));
        router.verifyBatch(batch, digests);
    }

    function test_verifyBatch_revertsOnFirstSealSelectorTombstoned() public {
        _setupVerifierEcosystem();
        vm.prank(ADMIN);
        router.removeEntry(V_ENTRY);

        FulfillmentBatch memory batch = _verifierBatch(1);
        bytes32[] memory digests = new bytes32[](1);
        vm.expectRevert(abi.encodeWithSelector(BoundlessRouter.EntryRemoved.selector, V_ENTRY));
        router.verifyBatch(batch, digests);
    }

    function test_verifyBatch_revertsOnFirstSealSelectorIsClass() public {
        _setupVerifierEcosystem();
        FulfillmentBatch memory batch = _verifierBatch(1);
        batch.fills[0].seal = abi.encodePacked(V_CLASS, hex"deadbeef");
        // Match the signed selector to the seal selector so we don't hit a
        // signed-selector revert before reaching the EntryIsClass check.
        batch.requests[0].selector = V_CLASS;
        bytes32[] memory digests = new bytes32[](1);
        vm.expectRevert(abi.encodeWithSelector(BoundlessRouter.EntryIsClass.selector, V_CLASS));
        router.verifyBatch(batch, digests);
    }

    // The dispatch-side `ClassRemoved` branch in `_classTagOf` is now
    // unreachable via the public API — `removeClass` requires the class to
    // have no live entries, so an entry whose `classId` resolves to a
    // tombstoned class cannot exist. The branch remains as defense in depth.

    function test_verifyBatch_revertsOnTerminalAssessorAsVerifier() public {
        _setupVerifierEcosystem();
        FulfillmentBatch memory batch = _verifierBatch(1);
        // The assessor entry exists — point the per-fill seal at it. The
        // router's first-seal resolution sees an assessor-class entry as the
        // verifier candidate and rejects it.
        batch.fills[0].seal = abi.encodePacked(A_ENTRY, hex"deadbeef");
        batch.requests[0].selector = A_ENTRY;
        bytes32[] memory digests = new bytes32[](1);
        vm.expectRevert(abi.encodeWithSelector(BoundlessRouter.TerminalAssessorAsVerifier.selector, A_CLASS));
        router.verifyBatch(batch, digests);
    }

    // ─── B.3 Per-fill verifier-class dispatch ─────────────────────────────

    function test_verifier_singleFill_callsVerifierAndAssessor() public {
        _setupVerifierEcosystem();
        FulfillmentBatch memory batch = _verifierBatch(1);
        bytes32[] memory digests = new bytes32[](1);

        // Verifier called once with the seal + claimDigest forwarded verbatim.
        vm.expectCall(
            verifierImpl,
            abi.encodeCall(IBoundlessVerifier.verify, (batch.fills[0].seal, batch.fills[0].claimDigest)),
            1
        );
        // Assessor called once (full calldata equality covered in §B.9; here we
        // only assert the call happened).
        vm.expectCall(assessorImpl, abi.encodeWithSelector(IBoundlessAssessor.verifyAssessor.selector), 1);

        router.verifyBatch(batch, digests);
    }

    function test_verifier_multiFill_sameSelector_cachedLookup() public {
        _setupVerifierEcosystem();
        uint64 n = 4;
        FulfillmentBatch memory batch = _verifierBatch(n);
        bytes32[] memory digests = new bytes32[](n);

        // Verifier called N times, once per fill.
        vm.expectCall(verifierImpl, abi.encodeWithSelector(IBoundlessVerifier.verify.selector), n);
        router.verifyBatch(batch, digests);
    }

    function test_verifier_multiFill_distinctEntriesSameClass_succeeds() public {
        _setupVerifierEcosystem();
        // A second verifier entry under the same class.
        address secondVerifier = address(new NullVerifier());
        _instantiateAsAdmin(V_ENTRY_2, secondVerifier, V_CLASS);

        FulfillmentBatch memory batch = _verifierBatch(2);
        // Switch fill 1 to V_ENTRY_2.
        batch.fills[1].seal = abi.encodePacked(V_ENTRY_2, hex"deadbeef");
        batch.requests[1].selector = V_ENTRY_2;

        bytes32[] memory digests = new bytes32[](2);

        // Each verifier impl called exactly once.
        vm.expectCall(verifierImpl, abi.encodeWithSelector(IBoundlessVerifier.verify.selector), 1);
        vm.expectCall(secondVerifier, abi.encodeWithSelector(IBoundlessVerifier.verify.selector), 1);

        router.verifyBatch(batch, digests);
    }

    function test_verifier_forwardsSealAndClaimDigestVerbatim() public {
        _setupVerifierEcosystem();
        FulfillmentBatch memory batch = _verifierBatch(1);
        // Use a non-trivial seal payload to make sure the bytes round-trip.
        batch.fills[0].seal = abi.encodePacked(V_ENTRY, hex"00112233445566778899AABBCCDDEEFF");
        batch.fills[0].claimDigest = keccak256("claim-under-test");
        bytes32[] memory digests = new bytes32[](1);

        vm.expectCall(
            verifierImpl, abi.encodeCall(IBoundlessVerifier.verify, (batch.fills[0].seal, batch.fills[0].claimDigest))
        );
        router.verifyBatch(batch, digests);
    }

    function test_verifier_revertingAdapter_yieldsVerifierFailedAtCorrectIndex() public {
        // Replace the V_ENTRY impl with one that reverts, on a fresh setup so
        // we can pick which fill it fails at.
        _addAssessorClass(A_CLASS, false);
        _addVerifierClass(V_CLASS, A_CLASS, false, false);
        _instantiateAsAdmin(A_ENTRY, assessorImpl, A_CLASS);
        _instantiateAsAdmin(V_ENTRY, address(new NullVerifier()), V_CLASS);
        address boomImpl = address(new RevertingVerifier());
        _instantiateAsAdmin(V_ENTRY_2, boomImpl, V_CLASS);

        FulfillmentBatch memory batch = _verifierBatch(3);
        // Fail at index 1.
        batch.fills[1].seal = abi.encodePacked(V_ENTRY_2, hex"deadbeef");
        batch.requests[1].selector = V_ENTRY_2;
        bytes32[] memory digests = new bytes32[](3);

        vm.expectRevert(abi.encodeWithSelector(BoundlessRouter.VerifierFailed.selector, uint256(1), V_ENTRY_2));
        router.verifyBatch(batch, digests);
    }

    function test_verifier_gasHogAdapter_yieldsVerifierFailed() public {
        _addAssessorClass(A_CLASS, false);
        _addVerifierClass(V_CLASS, A_CLASS, false, false);
        _instantiateAsAdmin(A_ENTRY, assessorImpl, A_CLASS);
        // Pin the gas-hog impl at a low explicit gas limit so the router's
        // try/catch traps the OOG and converts it to VerifierFailed.
        address hog = address(new GasHogVerifier());
        _instantiateAs(ADMIN, V_ENTRY, hog, V_CLASS, 50_000);

        FulfillmentBatch memory batch = _verifierBatch(1);
        bytes32[] memory digests = new bytes32[](1);
        vm.expectRevert(abi.encodeWithSelector(BoundlessRouter.VerifierFailed.selector, uint256(0), V_ENTRY));
        router.verifyBatch(batch, digests);
    }

    // ─── B.4 Per-fill joint-class dispatch ────────────────────────────────

    function test_joint_singleFill_callsJointAdapter() public {
        _setupJointEcosystem();
        FulfillmentBatch memory batch = _jointBatch(1);
        bytes32[] memory digests = new bytes32[](1);
        digests[0] = keccak256("req-digest-0");

        // Joint adapter called once with the full slim/fill/digest/prover tuple
        // forwarded verbatim. Per-property assertions (empty-seal acceptance,
        // no-assessor-call) live in their own tests below.
        vm.expectCall(
            jointImpl,
            abi.encodeCall(
                IBoundlessJointVerifierAssessor.verifyJoint,
                (batch.requests[0], batch.fills[0], digests[0], batch.prover)
            ),
            1
        );
        router.verifyBatch(batch, digests);
    }

    function test_joint_revertsOnNonEmptyAssessorSeal() public {
        _setupJointEcosystem();
        FulfillmentBatch memory batch = _jointBatch(1);
        batch.assessorSeal = hex"deadbeef";
        bytes32[] memory digests = new bytes32[](1);
        vm.expectRevert(BoundlessRouter.AssessorMustBeAbsent.selector);
        router.verifyBatch(batch, digests);
    }

    function test_joint_succeedsWithEmptyAssessorSeal() public {
        _setupJointEcosystem();
        FulfillmentBatch memory batch = _jointBatch(1);
        assertEq(batch.assessorSeal.length, 0);
        bytes32[] memory digests = new bytes32[](1);
        router.verifyBatch(batch, digests);
    }

    function test_joint_doesNotCallAnyAssessor() public {
        _setupJointEcosystem();
        // Register an assessor too — we want to make sure even if one exists,
        // joint dispatch does not invoke it.
        _addAssessorClass(A_CLASS, false);
        _instantiateAsAdmin(A_ENTRY, assessorImpl, A_CLASS);

        FulfillmentBatch memory batch = _jointBatch(1);
        bytes32[] memory digests = new bytes32[](1);

        vm.expectCall(assessorImpl, abi.encodeWithSelector(IBoundlessAssessor.verifyAssessor.selector), 0);
        router.verifyBatch(batch, digests);
    }

    function test_joint_revertingAdapter_yieldsVerifierFailedAtCorrectIndex() public {
        _addJointClass(J_CLASS, false);
        _instantiateAsAdmin(J_ENTRY, jointImpl, J_CLASS);
        bytes4 jointEntry2 = 0x00000032;
        address boom = address(new RevertingJoint());
        _instantiateAsAdmin(jointEntry2, boom, J_CLASS);

        FulfillmentBatch memory batch = _jointBatch(2);
        batch.fills[1].seal = abi.encodePacked(jointEntry2, hex"deadbeef");
        batch.requests[1].selector = jointEntry2;
        bytes32[] memory digests = new bytes32[](2);

        vm.expectRevert(abi.encodeWithSelector(BoundlessRouter.VerifierFailed.selector, uint256(1), jointEntry2));
        router.verifyBatch(batch, digests);
    }

    // ─── B.5 Mixed-class detection ────────────────────────────────────────

    function test_revertsOnMixedClasses_twoVerifierClasses() public {
        // Two verifier classes that share an assessor class.
        _setupVerifierEcosystem();
        bytes4 verifierClass2 = 0x00000013;
        bytes4 verifierEntry2 = 0x00000014;
        _addVerifierClass(verifierClass2, A_CLASS, false, false);
        _instantiateAsAdmin(verifierEntry2, address(new NullVerifier()), verifierClass2);

        FulfillmentBatch memory batch = _verifierBatch(2);
        batch.fills[1].seal = abi.encodePacked(verifierEntry2, hex"deadbeef");
        batch.requests[1].selector = verifierEntry2;
        bytes32[] memory digests = new bytes32[](2);

        // Single-class-per-batch is structural: the router hoists the dispatch
        // interface tag out of the per-fill loop. Mixing verifier classes —
        // even if they share an assessor class — breaks that hoist, so the
        // router rejects the batch.
        vm.expectRevert(abi.encodeWithSelector(BoundlessRouter.MixedClassWithinBatch.selector, V_CLASS, verifierClass2));
        router.verifyBatch(batch, digests);
    }

    function test_revertsOnMixedClasses_verifierThenJoint() public {
        _setupVerifierEcosystem();
        _setupJointEcosystem();
        FulfillmentBatch memory batch = _verifierBatch(2);
        // Replace fill 1 with a joint-class seal.
        batch.fills[1].seal = abi.encodePacked(J_ENTRY, hex"deadbeef");
        batch.requests[1].selector = J_ENTRY;
        bytes32[] memory digests = new bytes32[](2);

        vm.expectRevert(abi.encodeWithSelector(BoundlessRouter.MixedClassWithinBatch.selector, V_CLASS, J_CLASS));
        router.verifyBatch(batch, digests);
    }

    function test_revertsOnMidBatchTombstonedEntry() public {
        _setupVerifierEcosystem();
        address v2 = address(new NullVerifier());
        _instantiateAsAdmin(V_ENTRY_2, v2, V_CLASS);
        // Tombstone V_ENTRY_2; the batch's fill 1 references it.
        vm.prank(ADMIN);
        router.removeEntry(V_ENTRY_2);

        FulfillmentBatch memory batch = _verifierBatch(2);
        batch.fills[1].seal = abi.encodePacked(V_ENTRY_2, hex"deadbeef");
        batch.requests[1].selector = V_ENTRY_2;
        bytes32[] memory digests = new bytes32[](2);

        vm.expectRevert(abi.encodeWithSelector(BoundlessRouter.EntryRemoved.selector, V_ENTRY_2));
        router.verifyBatch(batch, digests);
    }

    function test_revertsOnMidBatchUnknownEntry() public {
        _setupVerifierEcosystem();
        bytes4 ghost = 0x00000099;
        FulfillmentBatch memory batch = _verifierBatch(2);
        batch.fills[1].seal = abi.encodePacked(ghost, hex"deadbeef");
        batch.requests[1].selector = ghost;
        bytes32[] memory digests = new bytes32[](2);

        vm.expectRevert(abi.encodeWithSelector(BoundlessRouter.EntryUnknown.selector, ghost));
        router.verifyBatch(batch, digests);
    }

    // ─── B.6 Assessor dispatch ────────────────────────────────────────────

    function test_assessor_revertsOnEmptyAssessorSeal() public {
        _setupVerifierEcosystem();
        FulfillmentBatch memory batch = _verifierBatch(1);
        batch.assessorSeal = bytes("");
        bytes32[] memory digests = new bytes32[](1);
        vm.expectRevert(BoundlessRouter.AssessorRequired.selector);
        router.verifyBatch(batch, digests);
    }

    function test_assessor_revertsOnAssessorSealShorterThan4Bytes() public {
        _setupVerifierEcosystem();
        FulfillmentBatch memory batch = _verifierBatch(1);
        batch.assessorSeal = hex"010203"; // 3 bytes — short.
        bytes32[] memory digests = new bytes32[](1);
        vm.expectRevert(BoundlessRouter.MalformedSeal.selector);
        router.verifyBatch(batch, digests);
    }

    function test_assessor_assessorSealExactly4Bytes_succeeds() public {
        _setupVerifierEcosystem();
        FulfillmentBatch memory batch = _verifierBatch(1);
        batch.assessorSeal = abi.encodePacked(A_ENTRY); // exactly 4 bytes
        bytes32[] memory digests = new bytes32[](1);
        // Assessor is invoked with calldata-tail forwarded; no revert.
        vm.expectCall(assessorImpl, abi.encodeWithSelector(IBoundlessAssessor.verifyAssessor.selector));
        router.verifyBatch(batch, digests);
    }

    function test_assessor_revertsOnAssessorSelectorUnknown() public {
        _setupVerifierEcosystem();
        bytes4 ghost = 0x000000FE;
        FulfillmentBatch memory batch = _verifierBatch(1);
        batch.assessorSeal = abi.encodePacked(ghost, hex"cafe");
        bytes32[] memory digests = new bytes32[](1);
        vm.expectRevert(abi.encodeWithSelector(BoundlessRouter.EntryUnknown.selector, ghost));
        router.verifyBatch(batch, digests);
    }

    function test_assessor_revertsOnAssessorSelectorTombstoned() public {
        _setupVerifierEcosystem();
        vm.prank(ADMIN);
        router.removeEntry(A_ENTRY);

        FulfillmentBatch memory batch = _verifierBatch(1);
        bytes32[] memory digests = new bytes32[](1);
        vm.expectRevert(abi.encodeWithSelector(BoundlessRouter.EntryRemoved.selector, A_ENTRY));
        router.verifyBatch(batch, digests);
    }

    function test_assessor_revertsOnAssessorSelectorIsClass() public {
        _setupVerifierEcosystem();
        FulfillmentBatch memory batch = _verifierBatch(1);
        // Use A_CLASS (a class id) as the assessor selector.
        batch.assessorSeal = abi.encodePacked(A_CLASS, hex"cafe");
        bytes32[] memory digests = new bytes32[](1);
        vm.expectRevert(abi.encodeWithSelector(BoundlessRouter.EntryIsClass.selector, A_CLASS));
        router.verifyBatch(batch, digests);
    }

    function test_assessor_revertsOnAssessorSelectorIsAVerifierEntry() public {
        _setupVerifierEcosystem();
        FulfillmentBatch memory batch = _verifierBatch(1);
        // Point the assessor seal at the verifier entry — wrong class.
        batch.assessorSeal = abi.encodePacked(V_ENTRY, hex"cafe");
        bytes32[] memory digests = new bytes32[](1);
        vm.expectRevert(abi.encodeWithSelector(BoundlessRouter.AssessorClassMismatch.selector, A_CLASS, V_CLASS));
        router.verifyBatch(batch, digests);
    }

    function test_assessor_revertsOnAssessorWrongAssessorClass() public {
        // Two assessor classes. Verifier names class A, but seal points at
        // entry in class 2.
        _setupVerifierEcosystem();
        _addAssessorClass(A_CLASS_2, false);
        _instantiateAsAdmin(A_ENTRY_2, address(new NullAssessor()), A_CLASS_2);

        FulfillmentBatch memory batch = _verifierBatch(1);
        batch.assessorSeal = abi.encodePacked(A_ENTRY_2, hex"cafe");
        bytes32[] memory digests = new bytes32[](1);
        vm.expectRevert(abi.encodeWithSelector(BoundlessRouter.AssessorClassMismatch.selector, A_CLASS, A_CLASS_2));
        router.verifyBatch(batch, digests);
    }

    // ─── B.7 Dispatch ordering ────────────────────────────────────────────

    function test_jointWithBadAssessorSeal_perFillStillRuns() public {
        // Joint dispatch runs per-fill first, then checks assessorSeal is empty.
        // Confirm the joint impl IS called before the AssessorMustBeAbsent revert.
        // Mixed entries within the same joint class are exercised by
        // test_joint_revertingAdapter_yieldsVerifierFailedAtCorrectIndex.
        _setupJointEcosystem();
        FulfillmentBatch memory batch = _jointBatch(2);
        batch.assessorSeal = hex"deadbeef";
        bytes32[] memory digests = new bytes32[](2);

        vm.expectCall(jointImpl, abi.encodeWithSelector(IBoundlessJointVerifierAssessor.verifyJoint.selector), 2);
        vm.expectRevert(BoundlessRouter.AssessorMustBeAbsent.selector);
        router.verifyBatch(batch, digests);
    }

    function test_verifierWithMissingAssessorSeal_perFillStillRuns() public {
        // Verifier dispatch runs per-fill first, then checks assessorSeal is non-empty.
        // Confirm the verifier impl IS called before the AssessorRequired revert.
        _setupVerifierEcosystem();
        uint64 n = 3;
        FulfillmentBatch memory batch = _verifierBatch(n);
        batch.assessorSeal = bytes("");
        bytes32[] memory digests = new bytes32[](n);

        vm.expectCall(verifierImpl, abi.encodeWithSelector(IBoundlessVerifier.verify.selector), n);
        vm.expectRevert(BoundlessRouter.AssessorRequired.selector);
        router.verifyBatch(batch, digests);
    }

    // ─── B.8 Signed-selector resolution ───────────────────────────────────

    function test_signedSelector_acceptsExactEntryMatch() public {
        _setupVerifierEcosystem();
        FulfillmentBatch memory batch = _verifierBatch(1);
        batch.requests[0].selector = V_ENTRY; // matches seal selector exactly
        bytes32[] memory digests = new bytes32[](1);
        router.verifyBatch(batch, digests);
    }

    function test_signedSelector_acceptsClassMatch() public {
        _setupVerifierEcosystem();
        FulfillmentBatch memory batch = _verifierBatch(1);
        batch.requests[0].selector = V_CLASS; // matches seal's class id
        bytes32[] memory digests = new bytes32[](1);
        router.verifyBatch(batch, digests);
    }

    function test_signedSelector_acceptsChainDefault_whenDefaultIsSealClass() public {
        // Set V_CLASS as the chain default.
        _addAssessorClass(A_CLASS, false);
        _addVerifierClass(V_CLASS, A_CLASS, true, false);
        _instantiateAsAdmin(A_ENTRY, assessorImpl, A_CLASS);
        _instantiateAsAdmin(V_ENTRY, verifierImpl, V_CLASS);

        FulfillmentBatch memory batch = _verifierBatch(1);
        batch.requests[0].selector = bytes4(0); // chain-default sentinel
        bytes32[] memory digests = new bytes32[](1);
        router.verifyBatch(batch, digests);
    }

    function test_signedSelector_revertsOnChainDefault_whenNoDefault() public {
        _setupVerifierEcosystem(); // no isDefault flag set
        assertEq(router.defaultClassId(), bytes4(0));

        FulfillmentBatch memory batch = _verifierBatch(1);
        batch.requests[0].selector = bytes4(0);
        bytes32[] memory digests = new bytes32[](1);
        vm.expectRevert(BoundlessRouter.NoDefaultClass.selector);
        router.verifyBatch(batch, digests);
    }

    function test_signedSelector_revertsOnChainDefault_whenDefaultIsDifferentClass() public {
        // V_CLASS is registered (and used by the seal), but a *different* verifier
        // class V_CLASS_2 holds the chain-default flag.
        bytes4 V_CLASS_2 = 0x00000013;
        bytes4 V_ENTRY_DEFAULT = 0x00000014;
        _addAssessorClass(A_CLASS, false);
        _addVerifierClass(V_CLASS, A_CLASS, false, false);
        _addVerifierClass(V_CLASS_2, A_CLASS, true, false);
        _instantiateAsAdmin(A_ENTRY, assessorImpl, A_CLASS);
        _instantiateAsAdmin(V_ENTRY, verifierImpl, V_CLASS);
        _instantiateAsAdmin(V_ENTRY_DEFAULT, address(new NullVerifier()), V_CLASS_2);
        assertEq(router.defaultClassId(), V_CLASS_2);

        FulfillmentBatch memory batch = _verifierBatch(1); // seal points at V_ENTRY ∈ V_CLASS
        batch.requests[0].selector = bytes4(0); // chain-default sentinel
        bytes32[] memory digests = new bytes32[](1);
        vm.expectRevert(abi.encodeWithSelector(BoundlessRouter.SignedDefaultClassMismatch.selector, V_CLASS, V_CLASS_2));
        router.verifyBatch(batch, digests);
    }

    function test_signedSelector_revertsOnSignedClassMismatch() public {
        // Sign a different *class* id than the seal's class.
        bytes4 otherClass = 0x00000040;
        _setupVerifierEcosystem();
        _addAssessorClass(otherClass, false); // any registered class id will do

        FulfillmentBatch memory batch = _verifierBatch(1);
        batch.requests[0].selector = otherClass;
        bytes32[] memory digests = new bytes32[](1);
        vm.expectRevert(abi.encodeWithSelector(BoundlessRouter.SignedClassMismatch.selector, otherClass, V_CLASS));
        router.verifyBatch(batch, digests);
    }

    function test_signedSelector_revertsOnSignedEntryMismatch() public {
        _setupVerifierEcosystem();
        // The "Entry mismatch" path requires the signed bytes4 to resolve to a
        // live entry that is NOT the seal's entry AND NOT the seal's class id.
        // An entry under V_CLASS would short-circuit on the class-match branch
        // before reaching the entry-mismatch one, so the entry lives in a
        // separate verifier class V_CLASS_2.
        address impl2 = address(new NullVerifier());
        bytes4 otherEntry = 0x00000019;
        bytes4 V_CLASS_2 = 0x00000013;
        _addVerifierClass(V_CLASS_2, A_CLASS, false, false);
        _instantiateAsAdmin(otherEntry, impl2, V_CLASS_2);

        FulfillmentBatch memory batch = _verifierBatch(1);
        batch.requests[0].selector = otherEntry;
        bytes32[] memory digests = new bytes32[](1);
        vm.expectRevert(abi.encodeWithSelector(BoundlessRouter.SignedEntryMismatch.selector, otherEntry, V_ENTRY));
        router.verifyBatch(batch, digests);
    }

    function test_signedSelector_revertsOnSignedTombstoned_wasClass() public {
        _setupVerifierEcosystem();
        // Add and remove a class so its bytes4 is tombstoned.
        bytes4 deadClass = 0x00000040;
        _addAssessorClass(deadClass, false);
        vm.prank(ADMIN);
        router.removeClass(deadClass);

        FulfillmentBatch memory batch = _verifierBatch(1);
        batch.requests[0].selector = deadClass;
        bytes32[] memory digests = new bytes32[](1);
        vm.expectRevert(abi.encodeWithSelector(BoundlessRouter.SignedSelectorTombstoned.selector, deadClass));
        router.verifyBatch(batch, digests);
    }

    function test_signedSelector_revertsOnSignedTombstoned_wasEntry() public {
        _setupVerifierEcosystem();
        address impl2 = address(new NullVerifier());
        bytes4 deadEntry = 0x00000019;
        _instantiateAsAdmin(deadEntry, impl2, V_CLASS);
        vm.prank(ADMIN);
        router.removeEntry(deadEntry);

        FulfillmentBatch memory batch = _verifierBatch(1);
        batch.requests[0].selector = deadEntry;
        bytes32[] memory digests = new bytes32[](1);
        vm.expectRevert(abi.encodeWithSelector(BoundlessRouter.SignedSelectorTombstoned.selector, deadEntry));
        router.verifyBatch(batch, digests);
    }

    function test_signedSelector_revertsOnSignedSelectorUnknown() public {
        _setupVerifierEcosystem();
        bytes4 ghost = 0x00000055; // never registered
        FulfillmentBatch memory batch = _verifierBatch(1);
        batch.requests[0].selector = ghost;
        bytes32[] memory digests = new bytes32[](1);
        vm.expectRevert(abi.encodeWithSelector(BoundlessRouter.SignedSelectorUnknown.selector, ghost));
        router.verifyBatch(batch, digests);
    }

    function test_signedSelector_perFillIndependent() public {
        _setupVerifierEcosystem();
        FulfillmentBatch memory batch = _verifierBatch(2);
        batch.requests[0].selector = V_ENTRY; // exact entry match
        batch.requests[1].selector = V_CLASS; // class match — same fill, different signing
        bytes32[] memory digests = new bytes32[](2);
        router.verifyBatch(batch, digests);
    }

    /// @dev Fuzz: in a router with one verifier class + entry, any signed
    ///      selector ∈ {V_ENTRY, V_CLASS} must pass; any other bytes4
    ///      (other than 0 — handled by a dedicated test above) must revert.
    function testFuzz_signedSelector_acceptsExactAndClass(bytes4 signed) public {
        _setupVerifierEcosystem();

        FulfillmentBatch memory batch = _verifierBatch(1);
        batch.requests[0].selector = signed;
        bytes32[] memory digests = new bytes32[](1);

        if (signed == V_ENTRY || signed == V_CLASS) {
            // Happy path — must succeed.
            router.verifyBatch(batch, digests);
        } else if (signed == bytes4(0)) {
            // No default registered → revert.
            vm.expectRevert(BoundlessRouter.NoDefaultClass.selector);
            router.verifyBatch(batch, digests);
        } else {
            // Anything else must revert; we don't pin which specific error.
            vm.expectRevert();
            router.verifyBatch(batch, digests);
        }
    }

    // ─── B.9 Assessor forwarding (_forwardCalldataAsStaticCall) ───────────

    /// @dev The load-bearing ABI-stability invariant. The router strips its
    ///      own `verifyBatch` selector from the entry-point calldata and
    ///      prepends `verifyAssessor`'s selector before forwarding to the
    ///      assessor. The bytes the assessor observes must therefore equal
    ///      `abi.encodeCall(IBoundlessAssessor.verifyAssessor, (batch, digests))`
    ///      — i.e. an exact byte-for-byte calldata match.
    function test_assessorForward_calldataTailMatchesVerifyBatch() public {
        _setupVerifierEcosystem();
        FulfillmentBatch memory batch = _verifierBatch(2);
        bytes32[] memory digests = new bytes32[](2);
        digests[0] = keccak256("digest-0");
        digests[1] = keccak256("digest-1");

        bytes memory expected = abi.encodeCall(IBoundlessAssessor.verifyAssessor, (batch, digests));
        vm.expectCall(assessorImpl, expected, 1);
        router.verifyBatch(batch, digests);
    }

    function test_assessorForward_revertReasonBubblesVerbatim() public {
        // Replace the assessor impl with one that reverts with a known custom
        // error payload; assert the outer revert is byte-identical.
        _addAssessorClass(A_CLASS, false);
        _addVerifierClass(V_CLASS, A_CLASS, false, false);
        address boomAssessor = address(new RevertingAssessor());
        _instantiateAsAdmin(A_ENTRY, boomAssessor, A_CLASS);
        _instantiateAsAdmin(V_ENTRY, verifierImpl, V_CLASS);

        FulfillmentBatch memory batch = _verifierBatch(1);
        bytes32[] memory digests = new bytes32[](1);
        vm.expectRevert(abi.encodeWithSelector(RevertingAssessor.AssessorBoom.selector, uint256(0xDEADBEEF)));
        router.verifyBatch(batch, digests);
    }

    function test_assessorForward_emptyRevertBubblesAsEmpty() public {
        _addAssessorClass(A_CLASS, false);
        _addVerifierClass(V_CLASS, A_CLASS, false, false);
        address emptyAssessor = address(new EmptyRevertAssessor());
        _instantiateAsAdmin(A_ENTRY, emptyAssessor, A_CLASS);
        _instantiateAsAdmin(V_ENTRY, verifierImpl, V_CLASS);

        FulfillmentBatch memory batch = _verifierBatch(1);
        bytes32[] memory digests = new bytes32[](1);
        // Empty revert data — the outer revert must carry zero bytes too.
        vm.expectRevert(bytes(""));
        router.verifyBatch(batch, digests);
    }

    function test_assessorForward_gasCapHonored() public {
        _addAssessorClass(A_CLASS, false);
        _addVerifierClass(V_CLASS, A_CLASS, false, false);
        // Pin the gas-hog assessor at a small explicit gas limit so the
        // staticcall OOGs cleanly. The router forwards the empty OOG
        // returndata as an empty revert.
        address hog = address(new GasHogAssessor());
        _instantiateAs(ADMIN, A_ENTRY, hog, A_CLASS, 50_000);
        _instantiateAsAdmin(V_ENTRY, verifierImpl, V_CLASS);

        FulfillmentBatch memory batch = _verifierBatch(1);
        bytes32[] memory digests = new bytes32[](1);
        vm.expectRevert(bytes(""));
        router.verifyBatch(batch, digests);
    }
}
