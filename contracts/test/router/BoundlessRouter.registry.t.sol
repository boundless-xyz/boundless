// Copyright 2026 Boundless Foundation, Inc.
//
// Use of this source code is governed by the Business Source License
// as found in the LICENSE-BSL file.
// SPDX-License-Identifier: BUSL-1.1

pragma solidity ^0.8.26;

import {Test} from "forge-std/Test.sol";
import {IAccessControl} from "@openzeppelin/contracts/access/IAccessControl.sol";

import {RouterTestBase} from "./RouterTestBase.sol";
import {BoundlessRouter} from "../../src/router/BoundlessRouter.sol";
import {IBoundlessVerifier} from "../../src/router/interfaces/IBoundlessVerifier.sol";
import {IBoundlessJointVerifierAssessor} from "../../src/router/interfaces/IBoundlessJointVerifierAssessor.sol";
import {IBoundlessAssessor} from "../../src/router/interfaces/IBoundlessAssessor.sol";

import {
    NullVerifier,
    NullAssessor,
    NullJoint,
    NonErc165Verifier,
    MisreportingErc165Verifier,
    RevertingErc165Verifier,
    RevertingVerifier
} from "../mocks/RouterMocks.sol";

/// @title BoundlessRouterRegistryTest — Section A of the router test plan.
///
/// @notice Covers initialization + access control, class management, default
///         class state-machine, and entry management. No `verifyBatch`
///         dispatch tests (those live in `BoundlessRouter.dispatch.t.sol`).
contract BoundlessRouterRegistryTest is RouterTestBase {
    // Shared selectors / class ids used across tests.
    bytes4 internal constant V_CLASS = 0x00000010;
    bytes4 internal constant V_ENTRY = 0x00000011;
    bytes4 internal constant A_CLASS = 0x00000020;
    bytes4 internal constant A_ENTRY = 0x00000021;
    bytes4 internal constant J_CLASS = 0x00000030;
    bytes4 internal constant J_ENTRY = 0x00000031;

    // ─── A.1 Access control ───────────────────────────────────────────────

    function test_addClass_revertsForNonAdmin() public {
        vm.startPrank(USER);
        vm.expectRevert(
            abi.encodeWithSelector(IAccessControl.AccessControlUnauthorizedAccount.selector, USER, router.ADMIN_ROLE())
        );
        router.addClass(
            A_CLASS,
            BoundlessRouter.ClassMetadata({
                interfaceTag: type(IBoundlessAssessor).interfaceId,
                permissionlessInstantiate: false,
                isDefault: false,
                requiredAssessorClass: bytes4(0),
                schemaArtifact: bytes32(0),
                schemaArtifactUrl: "",
                defaultGasLimit: 10_000_000,
                label: ""
            })
        );
        vm.stopPrank();
    }

    function test_removeClass_revertsForNonAdmin() public {
        _addAssessorClass(A_CLASS, false);
        vm.startPrank(USER);
        vm.expectRevert(
            abi.encodeWithSelector(IAccessControl.AccessControlUnauthorizedAccount.selector, USER, router.ADMIN_ROLE())
        );
        router.removeClass(A_CLASS);
        vm.stopPrank();
    }

    function test_removeEntry_revertsForNonAdmin() public {
        _addAssessorClass(A_CLASS, false);
        _instantiateAsAdmin(A_ENTRY, address(new NullAssessor()), A_CLASS);
        vm.startPrank(USER);
        vm.expectRevert(
            abi.encodeWithSelector(IAccessControl.AccessControlUnauthorizedAccount.selector, USER, router.ADMIN_ROLE())
        );
        router.removeEntry(A_ENTRY);
        vm.stopPrank();
    }

    // ─── A.2 addClass happy paths ─────────────────────────────────────────

    function test_addClass_verifierClass() public {
        _addAssessorClass(A_CLASS, false);

        BoundlessRouter.ClassMetadata memory meta = BoundlessRouter.ClassMetadata({
            interfaceTag: type(IBoundlessVerifier).interfaceId,
            permissionlessInstantiate: false,
            isDefault: false,
            requiredAssessorClass: A_CLASS,
            schemaArtifact: bytes32(0),
            schemaArtifactUrl: "",
            defaultGasLimit: 100_000,
            label: ""
        });

        vm.expectEmit(true, false, false, true, address(router));
        emit BoundlessRouter.ClassAdded(V_CLASS, meta);

        vm.prank(ADMIN);
        router.addClass(V_CLASS, meta);

        (
            bytes4 tag,
            bool permissionless,
            bool isDefault,
            bytes4 requiredAssessor,
            bytes32 schemaArtifact,
            uint64 defaultGasLimit
        ) = _readClass(V_CLASS);
        assertEq(tag, type(IBoundlessVerifier).interfaceId);
        assertEq(permissionless, false);
        assertEq(isDefault, false);
        assertEq(requiredAssessor, A_CLASS);
        assertEq(schemaArtifact, bytes32(0));
        assertEq(defaultGasLimit, uint64(100_000));
        assertEq(router.defaultClassId(), bytes4(0));
    }

    function test_addClass_jointClass() public {
        BoundlessRouter.ClassMetadata memory meta = BoundlessRouter.ClassMetadata({
            interfaceTag: type(IBoundlessJointVerifierAssessor).interfaceId,
            permissionlessInstantiate: true,
            isDefault: false,
            requiredAssessorClass: bytes4(0),
            schemaArtifact: bytes32(0),
            schemaArtifactUrl: "",
            defaultGasLimit: 200_000,
            label: ""
        });

        vm.prank(ADMIN);
        router.addClass(J_CLASS, meta);

        (bytes4 tag,,, bytes4 requiredAssessor,, uint64 defaultGasLimit) = _readClass(J_CLASS);
        assertEq(tag, type(IBoundlessJointVerifierAssessor).interfaceId);
        assertEq(requiredAssessor, bytes4(0));
        assertEq(defaultGasLimit, uint64(200_000));
    }

    function test_addClass_assessorClass() public {
        BoundlessRouter.ClassMetadata memory meta = BoundlessRouter.ClassMetadata({
            interfaceTag: type(IBoundlessAssessor).interfaceId,
            permissionlessInstantiate: false,
            isDefault: false,
            requiredAssessorClass: bytes4(0),
            schemaArtifact: bytes32(0),
            schemaArtifactUrl: "",
            defaultGasLimit: 10_000_000,
            label: ""
        });

        vm.prank(ADMIN);
        router.addClass(A_CLASS, meta);

        (bytes4 tag,,, bytes4 requiredAssessor,,) = _readClass(A_CLASS);
        assertEq(tag, type(IBoundlessAssessor).interfaceId);
        assertEq(requiredAssessor, bytes4(0));
    }

    function test_addClass_storesAllFields() public {
        _addAssessorClass(A_CLASS, false);

        BoundlessRouter.ClassMetadata memory meta = BoundlessRouter.ClassMetadata({
            interfaceTag: type(IBoundlessVerifier).interfaceId,
            permissionlessInstantiate: true,
            isDefault: false,
            requiredAssessorClass: A_CLASS,
            schemaArtifact: keccak256("schema-v1"),
            schemaArtifactUrl: "https://example.test/schema-v1.json",
            defaultGasLimit: 1_234_567,
            label: "labelled-verifier-class"
        });

        // ClassAdded carries the full metadata including strings — assert the
        // emit matches byte-identically to round-trip string fields too.
        vm.expectEmit(true, false, false, true, address(router));
        emit BoundlessRouter.ClassAdded(V_CLASS, meta);

        vm.prank(ADMIN);
        router.addClass(V_CLASS, meta);

        (
            bytes4 tag,
            bool permissionless,
            bool isDefault,
            bytes4 requiredAssessor,
            bytes32 schemaArtifact,
            uint64 defaultGasLimit
        ) = _readClass(V_CLASS);
        assertEq(tag, meta.interfaceTag);
        assertEq(permissionless, true);
        assertEq(isDefault, false);
        assertEq(requiredAssessor, meta.requiredAssessorClass);
        assertEq(schemaArtifact, meta.schemaArtifact);
        assertEq(defaultGasLimit, meta.defaultGasLimit);
    }

    // ─── A.3 addClass error branches ──────────────────────────────────────

    function _verifierMeta(bytes4 assessorClassId, bool isDefault)
        private
        pure
        returns (BoundlessRouter.ClassMetadata memory)
    {
        return BoundlessRouter.ClassMetadata({
            interfaceTag: type(IBoundlessVerifier).interfaceId,
            permissionlessInstantiate: false,
            isDefault: isDefault,
            requiredAssessorClass: assessorClassId,
            schemaArtifact: bytes32(0),
            schemaArtifactUrl: "",
            defaultGasLimit: 100_000,
            label: ""
        });
    }

    function _jointMeta() private pure returns (BoundlessRouter.ClassMetadata memory) {
        return BoundlessRouter.ClassMetadata({
            interfaceTag: type(IBoundlessJointVerifierAssessor).interfaceId,
            permissionlessInstantiate: false,
            isDefault: false,
            requiredAssessorClass: bytes4(0),
            schemaArtifact: bytes32(0),
            schemaArtifactUrl: "",
            defaultGasLimit: 100_000,
            label: ""
        });
    }

    function _assessorMeta() private pure returns (BoundlessRouter.ClassMetadata memory) {
        return BoundlessRouter.ClassMetadata({
            interfaceTag: type(IBoundlessAssessor).interfaceId,
            permissionlessInstantiate: false,
            isDefault: false,
            requiredAssessorClass: bytes4(0),
            schemaArtifact: bytes32(0),
            schemaArtifactUrl: "",
            defaultGasLimit: 10_000_000,
            label: ""
        });
    }

    function test_addClass_revertsOnZeroSelector() public {
        _addAssessorClass(A_CLASS, false);
        vm.prank(ADMIN);
        vm.expectRevert(BoundlessRouter.ZeroSelectorReserved.selector);
        router.addClass(bytes4(0), _verifierMeta(A_CLASS, false));
    }

    function test_addClass_revertsOnTombstonedClassId() public {
        _addAssessorClass(A_CLASS, false);
        vm.prank(ADMIN);
        router.removeClass(A_CLASS);
        vm.prank(ADMIN);
        vm.expectRevert(abi.encodeWithSelector(BoundlessRouter.ClassRemoved.selector, A_CLASS));
        router.addClass(A_CLASS, _assessorMeta());
    }

    function test_addClass_revertsOnTombstonedFormerEntry() public {
        // Use A_ENTRY (non-reserved-prefix bytes4) as an entry first, tombstone it,
        // then try to register it as a class — the tombstone shares both namespaces.
        _addAssessorClass(A_CLASS, false);
        _instantiateAsAdmin(A_ENTRY, address(new NullAssessor()), A_CLASS);
        vm.prank(ADMIN);
        router.removeEntry(A_ENTRY);
        vm.prank(ADMIN);
        vm.expectRevert(abi.encodeWithSelector(BoundlessRouter.ClassRemoved.selector, A_ENTRY));
        router.addClass(A_ENTRY, _assessorMeta());
    }

    function test_addClass_revertsOnAlreadyRegisteredClass() public {
        _addAssessorClass(A_CLASS, false);
        vm.prank(ADMIN);
        vm.expectRevert(abi.encodeWithSelector(BoundlessRouter.ClassInUse.selector, A_CLASS));
        router.addClass(A_CLASS, _assessorMeta());
    }

    function test_addClass_revertsOnBytes4UsedAsEntry() public {
        _addAssessorClass(A_CLASS, false);
        _instantiateAsAdmin(A_ENTRY, address(new NullAssessor()), A_CLASS);
        vm.prank(ADMIN);
        vm.expectRevert(abi.encodeWithSelector(BoundlessRouter.EntryInUse.selector, A_ENTRY));
        router.addClass(A_ENTRY, _assessorMeta());
    }

    function test_addClass_revertsOnInvalidInterfaceTag() public {
        BoundlessRouter.ClassMetadata memory meta = _assessorMeta();
        meta.interfaceTag = bytes4(0xdeadbeef);
        vm.prank(ADMIN);
        vm.expectRevert(abi.encodeWithSelector(BoundlessRouter.InvalidInterfaceTag.selector, bytes4(0xdeadbeef)));
        router.addClass(A_CLASS, meta);
    }

    function test_addClass_revertsOnZeroDefaultGasLimit() public {
        BoundlessRouter.ClassMetadata memory meta = _assessorMeta();
        meta.defaultGasLimit = 0;
        vm.prank(ADMIN);
        vm.expectRevert(BoundlessRouter.InvalidGasLimit.selector);
        router.addClass(A_CLASS, meta);
    }

    function test_addClass_verifier_revertsOnZeroAssessorClass() public {
        vm.prank(ADMIN);
        vm.expectRevert(BoundlessRouter.AssessorClassRequired.selector);
        router.addClass(V_CLASS, _verifierMeta(bytes4(0), false));
    }

    function test_addClass_verifier_revertsOnUnknownAssessorClass() public {
        bytes4 ghost = bytes4(0x000000FE);
        vm.prank(ADMIN);
        vm.expectRevert(abi.encodeWithSelector(BoundlessRouter.ClassUnknown.selector, ghost));
        router.addClass(V_CLASS, _verifierMeta(ghost, false));
    }

    function test_addClass_verifier_revertsOnAssessorClassThatIsVerifier() public {
        // Use a verifier class as the named "assessor" — should reject.
        _addAssessorClass(A_CLASS, false);
        _addVerifierClass(V_CLASS, A_CLASS, false, false);
        bytes4 secondVerifier = 0x0000_00F1;
        vm.prank(ADMIN);
        vm.expectRevert(abi.encodeWithSelector(BoundlessRouter.AssessorClassNotAssessor.selector, V_CLASS));
        router.addClass(secondVerifier, _verifierMeta(V_CLASS, false));
    }

    function test_addClass_verifier_revertsOnAssessorClassThatIsJoint() public {
        _addJointClass(J_CLASS, false);
        vm.prank(ADMIN);
        vm.expectRevert(abi.encodeWithSelector(BoundlessRouter.AssessorClassNotAssessor.selector, J_CLASS));
        router.addClass(V_CLASS, _verifierMeta(J_CLASS, false));
    }

    function test_addClass_joint_revertsOnNonZeroAssessorClass() public {
        _addAssessorClass(A_CLASS, false);
        BoundlessRouter.ClassMetadata memory meta = _jointMeta();
        meta.requiredAssessorClass = A_CLASS;
        vm.prank(ADMIN);
        vm.expectRevert(BoundlessRouter.AssessorClassMustBeZero.selector);
        router.addClass(J_CLASS, meta);
    }

    function test_addClass_assessor_revertsOnNonZeroAssessorClass() public {
        _addAssessorClass(A_CLASS, false);
        bytes4 secondAssessor = 0x0000_00A2;
        BoundlessRouter.ClassMetadata memory meta = _assessorMeta();
        meta.requiredAssessorClass = A_CLASS;
        vm.prank(ADMIN);
        vm.expectRevert(BoundlessRouter.AssessorClassMustBeZero.selector);
        router.addClass(secondAssessor, meta);
    }

    // ─── A.4 Default-class state machine ──────────────────────────────────

    function test_addClass_setsFirstDefault() public {
        _addAssessorClass(A_CLASS, false);

        vm.expectEmit(true, true, false, false, address(router));
        emit BoundlessRouter.DefaultClassChanged(bytes4(0), V_CLASS);

        vm.prank(ADMIN);
        router.addClass(V_CLASS, _verifierMeta(A_CLASS, true));

        assertEq(router.defaultClassId(), V_CLASS);
    }

    function test_addClass_revertsOnSecondDefault() public {
        _addAssessorClass(A_CLASS, false);
        _addVerifierClass(V_CLASS, A_CLASS, true, false);

        bytes4 secondVerifier = 0x0000_00F1;
        vm.prank(ADMIN);
        vm.expectRevert(abi.encodeWithSelector(BoundlessRouter.DefaultClassExists.selector, V_CLASS));
        router.addClass(secondVerifier, _verifierMeta(A_CLASS, true));
    }

    function test_addClass_revertsOnNonVerifierDefault_joint() public {
        BoundlessRouter.ClassMetadata memory meta = _jointMeta();
        meta.isDefault = true;
        vm.prank(ADMIN);
        vm.expectRevert(BoundlessRouter.DefaultMustBeVerifier.selector);
        router.addClass(J_CLASS, meta);
    }

    function test_addClass_revertsOnNonVerifierDefault_assessor() public {
        BoundlessRouter.ClassMetadata memory meta = _assessorMeta();
        meta.isDefault = true;
        vm.prank(ADMIN);
        vm.expectRevert(BoundlessRouter.DefaultMustBeVerifier.selector);
        router.addClass(A_CLASS, meta);
    }

    function test_removeClass_clearsDefaultWhenRemovingDefault() public {
        _addAssessorClass(A_CLASS, false);
        _addVerifierClass(V_CLASS, A_CLASS, true, false);
        assertEq(router.defaultClassId(), V_CLASS);

        vm.expectEmit(true, true, false, false, address(router));
        emit BoundlessRouter.DefaultClassChanged(V_CLASS, bytes4(0));

        vm.prank(ADMIN);
        router.removeClass(V_CLASS);

        assertEq(router.defaultClassId(), bytes4(0));
    }

    function test_removeClass_doesNotClearDefaultWhenRemovingNonDefault() public {
        _addAssessorClass(A_CLASS, false);
        _addVerifierClass(V_CLASS, A_CLASS, true, false);
        bytes4 nonDefaultVerifier = 0x0000_00F1;
        _addVerifierClass(nonDefaultVerifier, A_CLASS, false, false);

        vm.prank(ADMIN);
        router.removeClass(nonDefaultVerifier);

        assertEq(router.defaultClassId(), V_CLASS);
    }

    function test_addClass_canSetNewDefaultAfterPriorDefaultRemoved() public {
        _addAssessorClass(A_CLASS, false);
        _addVerifierClass(V_CLASS, A_CLASS, true, false);
        vm.prank(ADMIN);
        router.removeClass(V_CLASS);

        bytes4 secondVerifier = 0x0000_00F1;
        vm.prank(ADMIN);
        router.addClass(secondVerifier, _verifierMeta(A_CLASS, true));

        assertEq(router.defaultClassId(), secondVerifier);
    }

    function test_addClass_cannotReuseRemovedDefaultClassId() public {
        _addAssessorClass(A_CLASS, false);
        _addVerifierClass(V_CLASS, A_CLASS, true, false);
        vm.prank(ADMIN);
        router.removeClass(V_CLASS);

        // Same bytes4 → tombstoned, even though it was the default.
        vm.prank(ADMIN);
        vm.expectRevert(abi.encodeWithSelector(BoundlessRouter.ClassRemoved.selector, V_CLASS));
        router.addClass(V_CLASS, _verifierMeta(A_CLASS, true));
    }

    // ─── A.5 removeClass ──────────────────────────────────────────────────

    function test_removeClass_tombstones() public {
        _addAssessorClass(A_CLASS, false);
        vm.prank(ADMIN);
        router.removeClass(A_CLASS);
        assertTrue(router.tombstoned(A_CLASS));
        (bytes4 tag,,,,,) = _readClass(A_CLASS);
        assertEq(tag, bytes4(0));
    }

    function test_removeClass_emitsClassTombstoned() public {
        _addAssessorClass(A_CLASS, false);
        vm.expectEmit(true, false, false, false, address(router));
        emit BoundlessRouter.ClassTombstoned(A_CLASS);
        vm.prank(ADMIN);
        router.removeClass(A_CLASS);
    }

    function test_removeClass_revertsOnUnknownClass() public {
        vm.prank(ADMIN);
        vm.expectRevert(abi.encodeWithSelector(BoundlessRouter.ClassUnknown.selector, A_CLASS));
        router.removeClass(A_CLASS);
    }

    function test_removeClass_revertsOnAlreadyRemoved() public {
        _addAssessorClass(A_CLASS, false);
        vm.prank(ADMIN);
        router.removeClass(A_CLASS);
        vm.prank(ADMIN);
        vm.expectRevert(abi.encodeWithSelector(BoundlessRouter.ClassUnknown.selector, A_CLASS));
        router.removeClass(A_CLASS);
    }

    function test_removeClass_revertsWhenEntriesStillPinned() public {
        _addAssessorClass(A_CLASS, false);
        _instantiateAsAdmin(A_ENTRY, address(new NullAssessor()), A_CLASS);

        vm.prank(ADMIN);
        vm.expectRevert(abi.encodeWithSelector(BoundlessRouter.ClassHasEntries.selector, A_CLASS, uint256(1)));
        router.removeClass(A_CLASS);
    }

    function test_removeClass_succeedsAfterAllEntriesRemoved() public {
        _addAssessorClass(A_CLASS, false);
        _instantiateAsAdmin(A_ENTRY, address(new NullAssessor()), A_CLASS);
        assertEq(router.entriesPerClass(A_CLASS), uint256(1));

        vm.prank(ADMIN);
        router.removeEntry(A_ENTRY);
        assertEq(router.entriesPerClass(A_CLASS), uint256(0));

        vm.prank(ADMIN);
        router.removeClass(A_CLASS);
        assertTrue(router.tombstoned(A_CLASS));
    }

    function test_entriesPerClass_tracksInstantiateAndRemoveEntry() public {
        _addAssessorClass(A_CLASS, false);
        assertEq(router.entriesPerClass(A_CLASS), uint256(0));

        _instantiateAsAdmin(A_ENTRY, address(new NullAssessor()), A_CLASS);
        assertEq(router.entriesPerClass(A_CLASS), uint256(1));

        bytes4 secondEntry = 0x0000_0029;
        _instantiateAsAdmin(secondEntry, address(new NullAssessor()), A_CLASS);
        assertEq(router.entriesPerClass(A_CLASS), uint256(2));

        vm.prank(ADMIN);
        router.removeEntry(A_ENTRY);
        assertEq(router.entriesPerClass(A_CLASS), uint256(1));

        vm.prank(ADMIN);
        router.removeEntry(secondEntry);
        assertEq(router.entriesPerClass(A_CLASS), uint256(0));
    }

    // ─── A.6 instantiate happy paths ──────────────────────────────────────

    // Selectors outside the reserved (0x00xxxxxx) prefix.
    bytes4 internal constant PUBLIC_ENTRY = 0xDEAD_BEEF;
    bytes4 internal constant PUBLIC_ENTRY_2 = 0xCAFE_BABE;

    function test_instantiate_byAdmin_underNonPermissionlessClass() public {
        _addAssessorClass(A_CLASS, false);
        address impl = address(new NullAssessor());
        vm.prank(ADMIN);
        router.instantiate(A_ENTRY, impl, A_CLASS, 0);
        (address storedImpl, bytes4 storedClassId,) = router.entries(A_ENTRY);
        assertEq(storedImpl, impl);
        assertEq(storedClassId, A_CLASS);
    }

    function test_instantiate_byUser_underPermissionlessClass_nonReservedPrefix() public {
        _addAssessorClass(A_CLASS, true);
        address impl = address(new NullAssessor());
        vm.prank(USER);
        router.instantiate(PUBLIC_ENTRY, impl, A_CLASS, 0);
        (address storedImpl,,) = router.entries(PUBLIC_ENTRY);
        assertEq(storedImpl, impl);
    }

    function test_instantiate_byAdmin_underPermissionlessClass_reservedPrefix() public {
        _addAssessorClass(A_CLASS, true);
        address impl = address(new NullAssessor());
        vm.prank(ADMIN);
        router.instantiate(A_ENTRY, impl, A_CLASS, 0);
        (address storedImpl,,) = router.entries(A_ENTRY);
        assertEq(storedImpl, impl);
    }

    function test_instantiate_storesEntry_andEmitsEntryAdded() public {
        _addAssessorClass(A_CLASS, false);
        address impl = address(new NullAssessor());

        vm.expectEmit(true, true, true, true, address(router));
        emit BoundlessRouter.EntryAdded(A_ENTRY, impl, A_CLASS, 10_000_000);

        vm.prank(ADMIN);
        router.instantiate(A_ENTRY, impl, A_CLASS, 0);
    }

    function test_instantiate_appliesDefaultGasLimitWhenZero() public {
        _addAssessorClass(A_CLASS, false);
        address impl = address(new NullAssessor());
        vm.prank(ADMIN);
        router.instantiate(A_ENTRY, impl, A_CLASS, 0);
        (,, uint64 gasLimit) = router.entries(A_ENTRY);
        assertEq(gasLimit, uint64(10_000_000));
    }

    function test_instantiate_appliesExplicitGasLimit() public {
        _addAssessorClass(A_CLASS, false);
        address impl = address(new NullAssessor());
        vm.prank(ADMIN);
        router.instantiate(A_ENTRY, impl, A_CLASS, 42_424_242);
        (,, uint64 gasLimit) = router.entries(A_ENTRY);
        assertEq(gasLimit, uint64(42_424_242));
    }

    // ─── A.7 instantiate error branches ───────────────────────────────────

    function test_instantiate_revertsOnZeroSelector() public {
        _addAssessorClass(A_CLASS, false);
        address impl = address(new NullAssessor());
        vm.prank(ADMIN);
        vm.expectRevert(BoundlessRouter.ZeroSelectorReserved.selector);
        router.instantiate(bytes4(0), impl, A_CLASS, 0);
    }

    function test_instantiate_revertsOnTombstonedSelector() public {
        _addAssessorClass(A_CLASS, false);
        address impl = address(new NullAssessor());
        _instantiateAsAdmin(A_ENTRY, impl, A_CLASS);
        vm.prank(ADMIN);
        router.removeEntry(A_ENTRY);
        vm.prank(ADMIN);
        vm.expectRevert(abi.encodeWithSelector(BoundlessRouter.EntryRemoved.selector, A_ENTRY));
        router.instantiate(A_ENTRY, impl, A_CLASS, 0);
    }

    function test_instantiate_revertsOnTombstonedFormerClassId() public {
        // Register A_CLASS as a class, remove it, then try to use that bytes4
        // as an *entry selector* — same tombstone applies to both namespaces.
        _addAssessorClass(A_CLASS, false);
        vm.prank(ADMIN);
        router.removeClass(A_CLASS);

        _addAssessorClass(0x0000_0099, false); // valid parent class for the call
        address impl = address(new NullAssessor());
        vm.prank(ADMIN);
        vm.expectRevert(abi.encodeWithSelector(BoundlessRouter.EntryRemoved.selector, A_CLASS));
        router.instantiate(A_CLASS, impl, 0x0000_0099, 0);
    }

    function test_instantiate_revertsOnSelectorUsedAsClass() public {
        _addAssessorClass(A_CLASS, false);
        address impl = address(new NullAssessor());
        vm.prank(ADMIN);
        vm.expectRevert(abi.encodeWithSelector(BoundlessRouter.ClassInUse.selector, A_CLASS));
        // Pass A_CLASS as the *selector* — collides with the class registration.
        router.instantiate(A_CLASS, impl, A_CLASS, 0);
    }

    function test_instantiate_revertsOnDuplicateSelector() public {
        _addAssessorClass(A_CLASS, false);
        _instantiateAsAdmin(A_ENTRY, address(new NullAssessor()), A_CLASS);
        address dupImpl = address(new NullAssessor());
        vm.prank(ADMIN);
        vm.expectRevert(abi.encodeWithSelector(BoundlessRouter.EntryInUse.selector, A_ENTRY));
        router.instantiate(A_ENTRY, dupImpl, A_CLASS, 0);
    }

    function test_instantiate_revertsOnZeroImpl() public {
        _addAssessorClass(A_CLASS, false);
        vm.prank(ADMIN);
        vm.expectRevert(abi.encodeWithSelector(BoundlessRouter.Erc165CheckFailed.selector, address(0), bytes4(0)));
        router.instantiate(A_ENTRY, address(0), A_CLASS, 0);
    }

    function test_instantiate_revertsOnUnknownParentClass() public {
        bytes4 ghost = 0x0000_00FE;
        address impl = address(new NullAssessor());
        vm.prank(ADMIN);
        vm.expectRevert(abi.encodeWithSelector(BoundlessRouter.ClassUnknown.selector, ghost));
        router.instantiate(A_ENTRY, impl, ghost, 0);
    }

    function test_instantiate_revertsOnRemovedParentClass() public {
        _addAssessorClass(A_CLASS, false);
        vm.prank(ADMIN);
        router.removeClass(A_CLASS);
        address impl = address(new NullAssessor());
        vm.prank(ADMIN);
        vm.expectRevert(abi.encodeWithSelector(BoundlessRouter.ClassUnknown.selector, A_CLASS));
        router.instantiate(A_ENTRY, impl, A_CLASS, 0);
    }

    function test_instantiate_revertsForNonAdminOnNonPermissionlessClass() public {
        _addAssessorClass(A_CLASS, false);
        address impl = address(new NullAssessor());
        vm.startPrank(USER);
        vm.expectRevert(
            abi.encodeWithSelector(IAccessControl.AccessControlUnauthorizedAccount.selector, USER, router.ADMIN_ROLE())
        );
        router.instantiate(PUBLIC_ENTRY, impl, A_CLASS, 0);
        vm.stopPrank();
    }

    function test_instantiate_revertsForNonAdminOnReservedPrefix_permissionless() public {
        // TODO: this is weird no? now we registered a class with a reserved prefix as permissionless?
        _addAssessorClass(A_CLASS, true);
        // A_ENTRY = 0x00000021 → starts with 0x00 → reserved-prefix → admin-only
        // even on a permissionless class.
        address impl = address(new NullAssessor());
        vm.startPrank(USER);
        vm.expectRevert(abi.encodeWithSelector(BoundlessRouter.ReservedPrefix.selector, A_ENTRY));
        router.instantiate(A_ENTRY, impl, A_CLASS, 0);
        vm.stopPrank();
    }

    function test_instantiate_revertsOnImplFailingErc165_returnsFalse() public {
        // NullVerifier returns false for IBoundlessAssessor → registering it
        // under an assessor class fails the ERC-165 check.
        _addAssessorClass(A_CLASS, false);
        address mismatchedImpl = address(new NullVerifier());
        vm.prank(ADMIN);
        vm.expectRevert(
            abi.encodeWithSelector(
                BoundlessRouter.Erc165CheckFailed.selector, mismatchedImpl, type(IBoundlessAssessor).interfaceId
            )
        );
        router.instantiate(A_ENTRY, mismatchedImpl, A_CLASS, 0);
    }

    function test_instantiate_revertsOnImplReverting_inErc165() public {
        _addAssessorClass(A_CLASS, false);
        _addVerifierClass(V_CLASS, A_CLASS, false, false);
        address impl = address(new RevertingErc165Verifier());
        vm.prank(ADMIN);
        vm.expectRevert(
            abi.encodeWithSelector(
                BoundlessRouter.Erc165CheckFailed.selector, impl, type(IBoundlessVerifier).interfaceId
            )
        );
        router.instantiate(V_ENTRY, impl, V_CLASS, 0);
    }

    function test_instantiate_revertsOnImplWithoutErc165() public {
        _addAssessorClass(A_CLASS, false);
        _addVerifierClass(V_CLASS, A_CLASS, false, false);
        address impl = address(new NonErc165Verifier());
        vm.prank(ADMIN);
        vm.expectRevert(
            abi.encodeWithSelector(
                BoundlessRouter.Erc165CheckFailed.selector, impl, type(IBoundlessVerifier).interfaceId
            )
        );
        router.instantiate(V_ENTRY, impl, V_CLASS, 0);
    }

    function test_instantiate_revertsOnImplReportingWrongTag() public {
        _addAssessorClass(A_CLASS, false);
        _addVerifierClass(V_CLASS, A_CLASS, false, false);
        // MisreportingErc165Verifier reports IBoundlessAssessor, not IBoundlessVerifier.
        address impl = address(new MisreportingErc165Verifier());
        vm.prank(ADMIN);
        vm.expectRevert(
            abi.encodeWithSelector(
                BoundlessRouter.Erc165CheckFailed.selector, impl, type(IBoundlessVerifier).interfaceId
            )
        );
        router.instantiate(V_ENTRY, impl, V_CLASS, 0);
    }

    // ─── A.8 removeEntry ──────────────────────────────────────────────────

    function test_removeEntry_clearsAndTombstones() public {
        _addAssessorClass(A_CLASS, false);
        _instantiateAsAdmin(A_ENTRY, address(new NullAssessor()), A_CLASS);
        vm.prank(ADMIN);
        router.removeEntry(A_ENTRY);
        (address impl,, uint64 gasLimit) = router.entries(A_ENTRY);
        assertEq(impl, address(0));
        assertEq(gasLimit, uint64(0));
        assertTrue(router.tombstoned(A_ENTRY));
    }

    function test_removeEntry_emitsEntryTombstoned() public {
        _addAssessorClass(A_CLASS, false);
        _instantiateAsAdmin(A_ENTRY, address(new NullAssessor()), A_CLASS);
        vm.expectEmit(true, false, false, false, address(router));
        emit BoundlessRouter.EntryTombstoned(A_ENTRY);
        vm.prank(ADMIN);
        router.removeEntry(A_ENTRY);
    }

    function test_removeEntry_revertsOnUnknownSelector() public {
        vm.prank(ADMIN);
        vm.expectRevert(abi.encodeWithSelector(BoundlessRouter.EntryUnknown.selector, A_ENTRY));
        router.removeEntry(A_ENTRY);
    }

    function test_removeEntry_revertsOnAlreadyRemoved() public {
        _addAssessorClass(A_CLASS, false);
        _instantiateAsAdmin(A_ENTRY, address(new NullAssessor()), A_CLASS);
        vm.prank(ADMIN);
        router.removeEntry(A_ENTRY);
        vm.prank(ADMIN);
        vm.expectRevert(abi.encodeWithSelector(BoundlessRouter.EntryUnknown.selector, A_ENTRY));
        router.removeEntry(A_ENTRY);
    }

    function test_removeEntry_revertsOnClassSelector() public {
        _addAssessorClass(A_CLASS, false);
        vm.prank(ADMIN);
        vm.expectRevert(abi.encodeWithSelector(BoundlessRouter.EntryUnknown.selector, A_CLASS));
        router.removeEntry(A_CLASS);
    }

    /// @dev Wraps the public mapping's auto-getter and discards the two string
    ///      fields most callers don't need. The auto-getter does return them,
    ///      but `expectEmit(... metadata)` already covers string round-trip
    ///      via the `ClassAdded` payload.
    function _readClass(bytes4 classId)
        private
        view
        returns (
            bytes4 interfaceTag,
            bool permissionlessInstantiate,
            bool isDefault,
            bytes4 requiredAssessorClass,
            bytes32 schemaArtifact,
            uint64 defaultGasLimit
        )
    {
        (
            interfaceTag, permissionlessInstantiate, isDefault, requiredAssessorClass, schemaArtifact,, defaultGasLimit,
        ) = router.classes(classId);
    }
}
