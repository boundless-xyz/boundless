// Copyright 2026 Boundless Foundation, Inc.
//
// Use of this source code is governed by the Business Source License
// as found in the LICENSE-BSL file.
// SPDX-License-Identifier: BUSL-1.1

pragma solidity ^0.8.26;

import {AccessControlUpgradeable} from "@openzeppelin/contracts-upgradeable/access/AccessControlUpgradeable.sol";
import {Initializable} from "@openzeppelin/contracts-upgradeable/proxy/utils/Initializable.sol";
import {UUPSUpgradeable} from "@openzeppelin/contracts-upgradeable/proxy/utils/UUPSUpgradeable.sol";
import {IERC165} from "@openzeppelin/contracts/utils/introspection/IERC165.sol";

import {IBoundlessVerifier} from "./interfaces/IBoundlessVerifier.sol";
import {IBoundlessJointVerifierAssessor} from "./interfaces/IBoundlessJointVerifierAssessor.sol";
import {IBoundlessAssessor} from "./interfaces/IBoundlessAssessor.sol";
import {SlimRequest} from "../types/SlimRequest.sol";
import {Fulfillment} from "../types/Fulfillment.sol";

/// @title BoundlessRouter — verification engine for the Boundless market.
///
/// @notice Owns the per-class verification dispatch. The market calls `verifySubBatch`
///         once per single-class sub-batch; the router resolves the verifier class from
///         the seals' first-4-byte selector, validates the requestor's signed selector,
///         dispatches per-fill into the right interface (`IBoundlessVerifier` or
///         `IBoundlessJointVerifierAssessor`), and dispatches once per sub-batch into the
///         class's required `IBoundlessAssessor` (when the verifier class is per-fill).
///
/// @dev    Two-mapping registry:
///           * `entries[selector]`  — concrete impl pin (one bytes4 → one contract).
///           * `classes[classId]`   — conformance group metadata (interface tag, required
///                                    assessor class, gas defaults, schema artifact, etc.).
///         Both maps share the same `bytes4` namespace. Namespace invariants enforce
///         mutual exclusion (no bytes4 in both maps), permanent tombstoning of removed
///         values, and the `0x00000000` reserved sentinel — so an EIP-712-signed request
///         can never be silently repointed by a later registration.
contract BoundlessRouter is Initializable, AccessControlUpgradeable, UUPSUpgradeable {
    /// @dev The version of the router contract, with respect to upgrades.
    uint64 public constant VERSION = 1;

    /// @notice Admin role identifier (governance).
    bytes32 public constant ADMIN_ROLE = DEFAULT_ADMIN_ROLE;

    /// @notice Reserved-prefix mask. A bytes4 in the range `0x00xxxxxx` is governance-only
    ///         namespace — permissionless `instantiate` rejects it.
    /// @dev    Combined with the `0x00000000` chain-default sentinel, this reserves
    ///         `0x00000001`–`0x00ffffff` for governance-curated registrations.
    bytes4 public constant RESERVED_PREFIX_MASK = 0xff000000;

    /// @notice Sentinel signed selector meaning "use the chain default class". A request
    ///         signed against this value matches any entry in the class flagged
    ///         `isDefault == true`. Cannot itself be registered.
    bytes4 public constant CHAIN_DEFAULT_SENTINEL = 0x00000000;

    /// @notice One concrete verifier / joint-verifier / assessor implementation pinned at
    ///         a given selector.
    struct Entry {
        /// @notice Address of the contract implementing the class's `interfaceTag`.
        address impl;
        /// @notice Class this entry belongs to. The class supplies the dispatch interface
        ///         tag and any binding metadata.
        bytes4 classId;
        /// @notice Per-call gas cap for `staticcall`s into `impl`. A misbehaving adapter
        ///         can self-rug its sub-batch on gas, but cannot starve settlement of
        ///         sibling sub-batches in the same transaction.
        uint64 gasLimit;
    }

    /// @notice Per-class conformance metadata.
    struct ClassMetadata {
        /// @notice ERC-165 interface id of the dispatch interface for impls under this
        ///         class. Must be one of the three accepted values; validated at
        ///         `addClass`. Also serves as a non-zero existence flag.
        bytes4 interfaceTag;
        /// @notice Whether anyone can `instantiate` an entry under this class.
        bool permissionlessInstantiate;
        /// @notice The chain-default class iff true. Exactly one class is the default at
        ///         any time. Governance-controlled. The default class's `interfaceTag`
        ///         must be `IBoundlessVerifier`.
        bool isDefault;
        /// @notice For verifier classes, the assessor class that binds requests to claims.
        ///         Must be a registered class with `interfaceTag == IBoundlessAssessor`.
        ///         Must be `0x00000000` for joint and terminal-assessor classes.
        bytes4 requiredAssessorClass;
        /// @notice Hash of the per-class conformance spec (claim-digest formula, journal
        ///         shape, seal layout, class invariants). Tooling fetches the artifact
        ///         off-chain and verifies its content hash against this on-chain commit.
        bytes32 schemaArtifact;
        /// @notice Optional URL where the schema artifact is published. Convenience
        ///         pointer; not authoritative — `schemaArtifact` is what binds the spec.
        string schemaArtifactUrl;
        /// @notice Default per-call gas cap applied at `instantiate` time when the caller
        ///         passes `gasLimit == 0`.
        uint64 defaultGasLimit;
        /// @notice Human-readable label.
        string label;
    }

    /// @notice Selector → impl pin.
    mapping(bytes4 => Entry) public entries;
    /// @notice Class id → class metadata.
    mapping(bytes4 => ClassMetadata) public classes;
    /// @notice Tombstone state for previously registered selectors / class ids. Once set,
    ///         a bytes4 cannot be reused for a different class or impl ever again — same
    ///         pattern as `RiscZeroVerifierRouter`. Critical because EIP-712-signed
    ///         requests in flight may still reference a removed value.
    mapping(bytes4 => bool) public tombstoned;
    /// @notice Cached chain-default class id for O(1) lookup. Mirrors whichever class has
    ///         `isDefault == true`. `0x00000000` if none.
    bytes4 public defaultClassId;

    // ─── Errors ────────────────────────────────────────────────────────────

    /// @notice Caller tried to register or look up `0x00000000`. That value is reserved
    ///         as the chain-default sentinel and can never appear in either map.
    error ZeroSelectorReserved();

    /// @notice A non-admin caller tried to instantiate an entry whose selector falls
    ///         under the governance-only reserved prefix (`0x00xxxxxx`).
    error ReservedPrefix(bytes4 selector);

    /// @notice Lookup or operation referenced a `classId` that was never registered.
    error ClassUnknown(bytes4 classId);

    /// @notice Caller tried to register a `classId` that is already in `classes` or
    ///         already in `entries` (the two namespaces are disjoint).
    error ClassInUse(bytes4 classId);

    /// @notice Caller tried to operate on a `classId` that has been tombstoned. The
    ///         bytes4 cannot be re-registered for any class or impl.
    error ClassRemoved(bytes4 classId);

    /// @notice Lookup or operation referenced a `selector` that was never registered as
    ///         an entry.
    error EntryUnknown(bytes4 selector);

    /// @notice Caller tried to register a `selector` that is already in `entries` or
    ///         already in `classes` (the two namespaces are disjoint).
    error EntryInUse(bytes4 selector);

    /// @notice Caller tried to operate on a `selector` that has been tombstoned. The
    ///         bytes4 cannot be re-registered for any class or impl.
    error EntryRemoved(bytes4 selector);

    /// @notice Class metadata declared an `interfaceTag` that isn't one of the three
    ///         accepted ERC-165 ids (`IBoundlessVerifier`, `IBoundlessJointVerifierAssessor`,
    ///         `IBoundlessAssessor`).
    error InvalidInterfaceTag(bytes4 interfaceTag);

    /// @notice A verifier-tagged class was registered without a `requiredAssessorClass`.
    ///         Verifier classes must always pair with an assessor class.
    error AssessorClassRequired();

    /// @notice A joint or assessor-tagged class was registered with a non-zero
    ///         `requiredAssessorClass`. Only verifier classes have an assessor seam.
    error AssessorClassMustBeZero();

    /// @notice A verifier class's `requiredAssessorClass` resolved to a class whose
    ///         `interfaceTag` is not the assessor interface.
    error AssessorClassNotAssessor(bytes4 classId);

    /// @notice An attempt was made to register a second class with `isDefault == true`.
    ///         Exactly one class may be the chain default at any time.
    error DefaultClassExists(bytes4 currentDefault);

    /// @notice An attempt was made to flag a non-verifier class as the chain default.
    ///         The default class must dispatch to `IBoundlessVerifier`.
    error DefaultMustBeVerifier();

    /// @notice Reserved for future symmetry with curated/permissionless gating. Currently
    ///         unused — non-permissionless paths revert via `AccessControl`.
    error PermissionlessNotAllowed(bytes4 classId);

    /// @notice An `instantiate` impl either failed `IERC165.supportsInterface(tag)` or
    ///         reverted on the call. Used as a unified error for "this address does not
    ///         conform to the class interface" — including the `address(0)` case.
    error Erc165CheckFailed(address impl, bytes4 expectedInterfaceId);

    /// @notice `verifySubBatch` was called with no fills.
    error EmptySubBatch();

    /// @notice The four per-fill input arrays passed to `verifySubBatch` are not the
    ///         same length.
    error LengthMismatch();

    /// @notice A seal in `verifySubBatch` was shorter than the 4 bytes required to
    ///         extract a selector.
    error MalformedSeal();

    /// @notice Two fills in the same sub-batch resolved to different verifier classes.
    ///         Each sub-batch must be single-class.
    error MixedClassWithinSubBatch(bytes4 expected, bytes4 received);

    /// @notice The requestor signed `0x00000000` (chain default), but no chain default
    ///         class is currently registered.
    error NoDefaultClass();

    /// @notice The requestor signed `0x00000000` (chain default), but the seal's class
    ///         is not the chain default class.
    error SignedDefaultClassMismatch(bytes4 sealClassId, bytes4 defaultClassId);

    /// @notice The requestor signed a registered class id, but the seal's class does
    ///         not match that class.
    error SignedClassMismatch(bytes4 signedClassId, bytes4 sealClassId);

    /// @notice The requestor signed a registered entry selector, but the seal's
    ///         selector does not match exactly.
    error SignedEntryMismatch(bytes4 signedSelector, bytes4 sealSelector);

    /// @notice The requestor signed a bytes4 that resolves to nothing — neither a
    ///         registered class nor a registered entry, and not tombstoned.
    error SignedSelectorUnknown(bytes4 signedSelector);

    /// @notice The requestor signed a bytes4 that has since been tombstoned. The router
    ///         refuses to service the request; brokers should drop such orders.
    error SignedSelectorTombstoned(bytes4 signed);

    /// @notice A per-fill verifier or joint adapter call reverted (or ran out of gas).
    ///         The failure is isolated to the offending fill's sub-batch — sibling
    ///         sub-batches in the same transaction still settle.
    error VerifierFailed(uint256 index, bytes4 selector);

    /// @notice The assessor selector supplied in `verifySubBatch` belongs to a class
    ///         other than the verifier class's `requiredAssessorClass`.
    error AssessorClassMismatch(bytes4 expected, bytes4 actual);

    /// @notice The first seal's class is itself an assessor class — assessor classes
    ///         are terminal and cannot be selected as the verifier class for a sub-batch.
    error TerminalAssessorAsVerifier(bytes4 classId);

    /// @notice A verifier-class sub-batch was submitted without an assessor selector.
    ///         The assessor seam is mandatory for verifier classes.
    error AssessorRequired();

    /// @notice A joint-class sub-batch was submitted with a non-empty assessor selector
    ///         or seal. Joint classes have no assessor seam — both fields must be zero.
    error AssessorMustBeAbsent();

    // ─── Events ────────────────────────────────────────────────────────────

    event ClassAdded(bytes4 indexed classId, ClassMetadata metadata);
    event ClassTombstoned(bytes4 indexed classId);
    event EntryAdded(bytes4 indexed selector, address indexed impl, bytes4 indexed classId, uint64 gasLimit);
    event EntryTombstoned(bytes4 indexed selector);
    event DefaultClassChanged(bytes4 indexed previousDefault, bytes4 indexed newDefault);

    /// @custom:oz-upgrades-unsafe-allow constructor
    constructor() {
        _disableInitializers();
    }

    function initialize(address admin) external initializer {
        __AccessControl_init();
        __UUPSUpgradeable_init();
        _grantRole(ADMIN_ROLE, admin);
    }

    function _authorizeUpgrade(address newImplementation) internal override onlyRole(ADMIN_ROLE) {}

    // ─── Registry: classes ────────────────────────────────────────────────

    /// @notice Add a new class. Governance-only.
    /// @dev    Enforces namespace invariants: `classId` is non-zero, non-tombstoned, and
    ///         not already in either map. Validates `interfaceTag` is one of the three
    ///         accepted values and that `requiredAssessorClass` matches the tag's
    ///         expectations (mandatory for verifier classes; absent otherwise).
    function addClass(bytes4 classId, ClassMetadata calldata metadata) external onlyRole(ADMIN_ROLE) {
        if (classId == CHAIN_DEFAULT_SENTINEL) revert ZeroSelectorReserved();
        if (tombstoned[classId]) revert ClassRemoved(classId);
        if (classes[classId].interfaceTag != bytes4(0)) revert ClassInUse(classId);
        if (entries[classId].impl != address(0)) revert EntryInUse(classId);

        bytes4 tag = metadata.interfaceTag;
        if (!_isVerifierTag(tag) && !_isJointTag(tag) && !_isAssessorTag(tag)) {
            revert InvalidInterfaceTag(tag);
        }

        if (_isVerifierTag(tag)) {
            // Verifier class: must name an assessor class that exists and is itself an assessor.
            if (metadata.requiredAssessorClass == bytes4(0)) revert AssessorClassRequired();
            ClassMetadata memory ac = classes[metadata.requiredAssessorClass];
            if (ac.interfaceTag == bytes4(0)) revert ClassUnknown(metadata.requiredAssessorClass);
            if (!_isAssessorTag(ac.interfaceTag)) {
                revert AssessorClassNotAssessor(metadata.requiredAssessorClass);
            }
        } else {
            // Joint or terminal-assessor: requiredAssessorClass must be zero.
            if (metadata.requiredAssessorClass != bytes4(0)) revert AssessorClassMustBeZero();
        }

        if (metadata.isDefault) {
            if (!_isVerifierTag(tag)) revert DefaultMustBeVerifier();
            if (defaultClassId != bytes4(0)) revert DefaultClassExists(defaultClassId);
            defaultClassId = classId;
            emit DefaultClassChanged(bytes4(0), classId);
        }

        classes[classId] = metadata;
        emit ClassAdded(classId, metadata);
    }

    /// @notice Remove a class. Governance-only. Tombstones the class id so it can never
    ///         be re-registered in either namespace.
    /// @dev    Removing a class does NOT remove its existing `entries`. Brokers and
    ///         clients should treat any entry whose `classId` resolves to a removed
    ///         class as unusable; the router's per-fill loop guards against this via the
    ///         class-existence check inside `_classOf`.
    function removeClass(bytes4 classId) external onlyRole(ADMIN_ROLE) {
        if (classes[classId].interfaceTag == bytes4(0)) revert ClassUnknown(classId);
        if (defaultClassId == classId) {
            defaultClassId = bytes4(0);
            emit DefaultClassChanged(classId, bytes4(0));
        }
        delete classes[classId];
        tombstoned[classId] = true;
        emit ClassTombstoned(classId);
    }

    // ─── Registry: entries ────────────────────────────────────────────────

    /// @notice Register a new impl entry under a class. Gated by the parent class's
    ///         `permissionlessInstantiate` flag — when false, only governance can call.
    ///
    /// @dev    Enforces:
    ///           * Selector non-zero, non-tombstoned, not registered as a class.
    ///           * Reserved-prefix policy: governance-only namespace
    ///             (`b4 & RESERVED_PREFIX_MASK == 0x00000000`) is rejected for
    ///             permissionless callers.
    ///           * Parent class exists and isn't tombstoned.
    ///           * `IERC165(impl).supportsInterface(parentClass.interfaceTag) == true`.
    ///         When `gasLimit == 0`, falls back to the parent class's `defaultGasLimit`.
    function instantiate(bytes4 selector, address impl, bytes4 parentClassId, uint64 gasLimit) external {
        if (selector == CHAIN_DEFAULT_SENTINEL) revert ZeroSelectorReserved();
        if (tombstoned[selector]) revert EntryRemoved(selector);
        if (classes[selector].interfaceTag != bytes4(0)) revert ClassInUse(selector);
        if (entries[selector].impl != address(0)) revert EntryInUse(selector);
        if (impl == address(0)) revert Erc165CheckFailed(impl, bytes4(0));

        ClassMetadata memory pc = classes[parentClassId];
        if (pc.interfaceTag == bytes4(0)) revert ClassUnknown(parentClassId);

        if (!pc.permissionlessInstantiate) {
            _checkRole(ADMIN_ROLE);
        } else if ((selector & RESERVED_PREFIX_MASK) == bytes4(0) && !hasRole(ADMIN_ROLE, msg.sender)) {
            revert ReservedPrefix(selector);
        }

        if (!_supportsInterface(impl, pc.interfaceTag)) revert Erc165CheckFailed(impl, pc.interfaceTag);

        uint64 effectiveGas = gasLimit == 0 ? pc.defaultGasLimit : gasLimit;
        entries[selector] = Entry({impl: impl, classId: parentClassId, gasLimit: effectiveGas});
        emit EntryAdded(selector, impl, parentClassId, effectiveGas);
    }

    /// @notice Remove an entry. Governance-only. Tombstones the selector.
    function removeEntry(bytes4 selector) external onlyRole(ADMIN_ROLE) {
        if (entries[selector].impl == address(0)) revert EntryUnknown(selector);
        delete entries[selector];
        tombstoned[selector] = true;
        emit EntryTombstoned(selector);
    }

    // ─── Verification engine ──────────────────────────────────────────────

    /// @notice Verify all fills in one single-class sub-batch.
    ///
    /// @param  requests        Per-fill `SlimRequest`. The CALLER is responsible
    ///                         for verifying each `SlimRequest` reconstructs to
    ///                         the lock's stored `requestDigest` before dispatch.
    ///                         The router and adapters trust the supplied payload.
    /// @param  fills           Per-fill `Fulfillment`. Same order as `requests`.
    ///                         Used for `claimDigest`, `seal`, and `fulfillmentData`.
    /// @param  requestDigests  Pre-computed `requestDigest` per fill, same order.
    ///                         Forwarded to the assessor adapter and to the
    ///                         joint per-fill dispatch so neither has to
    ///                         recompute it. The market builds this during the
    ///                         binding check; direct router callers must
    ///                         supply consistent values.
    /// @param  prover          Address the market will credit / slash for this
    ///                         sub-batch. Forwarded as a universal arg to the
    ///                         assessor / joint adapter, which is responsible
    ///                         for binding it via its own mechanism.
    /// @param  assessorSeal    Bytes for the assessor call (only used for
    ///                         verifier classes; must be empty for joint).
    ///                         First 4 bytes are the assessor selector for
    ///                         dispatch; the rest is forwarded to the assessor
    ///                         adapter.
    ///
    /// @dev    Per-fill calls are gas-bounded `staticcall`s wrapped in
    ///         try/catch — a malicious adapter can self-rug its sub-batch but
    ///         cannot starve settlement of sibling sub-batches. The function
    ///         is `view` because all dispatched calls are `staticcall`-equivalent.
    function verifySubBatch(
        SlimRequest[] calldata requests,
        Fulfillment[] calldata fills,
        bytes32[] calldata requestDigests,
        address prover,
        bytes calldata assessorSeal
    ) external view {
        uint256 n = fills.length;
        if (n == 0) revert EmptySubBatch();
        if (requests.length != n || requestDigests.length != n) revert LengthMismatch();

        // 1. Resolve the verifier class from the first seal.
        bytes4 firstSel = _sealSelector(fills[0].seal);
        Entry memory firstEntry = _entryOf(firstSel);
        bytes4 verifierClassId = firstEntry.classId;
        ClassMetadata memory cm = _classOf(verifierClassId);
        bytes4 tag = cm.interfaceTag;

        // A class registered with the assessor interface tag is terminal — only
        // referenced as `requiredAssessorClass`, never selected as a verifier class.
        if (_isAssessorTag(tag)) revert TerminalAssessorAsVerifier(verifierClassId);

        // 2. Per-fill loop: namespace check, signed-selector resolution, gas-bounded
        //    dispatch on interfaceTag.
        for (uint256 i = 0; i < n; i++) {
            bytes4 sealSel = _sealSelector(fills[i].seal);
            Entry memory e = _entryOf(sealSel);
            if (e.classId != verifierClassId) revert MixedClassWithinSubBatch(verifierClassId, e.classId);
            _matchSignedSelector(sealSel, requests[i].selector, verifierClassId);

            if (_isVerifierTag(tag)) {
                try IBoundlessVerifier(e.impl).verify{gas: e.gasLimit}(fills[i].seal, fills[i].claimDigest) {}
                catch {
                    revert VerifierFailed(i, sealSel);
                }
            } else if (_isJointTag(tag)) {
                try IBoundlessJointVerifierAssessor(e.impl).verifyJoint{gas: e.gasLimit}(
                    requestDigests[i], fills[i].claimDigest, prover, fills[i].seal
                ) {}
                catch {
                    revert VerifierFailed(i, sealSel);
                }
            } else {
                // Defensive: assessor tag was already excluded by `TerminalAssessorAsVerifier`,
                // and `addClass` rejects every other tag value. Reaching this branch means a
                // future interface was added to `addClass` without updating this dispatch.
                revert InvalidInterfaceTag(tag);
            }
        }

        // 3. Assessor dispatch — only for per-fill verifier classes.
        if (_isVerifierTag(tag)) {
            // Assessor seam mandatory for verifier classes. An empty seal signals
            // "missing"; anything else must start with a 4-byte assessor selector.
            if (assessorSeal.length == 0) revert AssessorRequired();
            bytes4 assessorSel = _sealSelector(assessorSeal);
            Entry memory asEntry = _entryOf(assessorSel);
            if (asEntry.classId != cm.requiredAssessorClass) {
                revert AssessorClassMismatch(cm.requiredAssessorClass, asEntry.classId);
            }
            IBoundlessAssessor(asEntry.impl).verifyAssessor{gas: asEntry.gasLimit}(
                requests, fills, requestDigests, prover, assessorSeal
            );
        } else if (_isJointTag(tag)) {
            // Joint class: no assessor seam — caller must signal that with an empty seal.
            if (assessorSeal.length != 0) revert AssessorMustBeAbsent();
        } else {
            // Defensive: see the per-fill dispatch above. Unreachable as long as
            // `addClass` and this function agree on the set of accepted interface tags.
            revert InvalidInterfaceTag(tag);
        }
    }

    // ─── Internal helpers ─────────────────────────────────────────────────

    /// @dev Extract the bytes4 selector from a seal's leading bytes.
    function _sealSelector(bytes calldata seal) internal pure returns (bytes4) {
        if (seal.length < 4) revert MalformedSeal();
        return bytes4(seal[0:4]);
    }

    /// @dev Look up an entry, reverting with the right error for unknown / tombstoned.
    function _entryOf(bytes4 selector) internal view returns (Entry memory e) {
        if (tombstoned[selector]) revert EntryRemoved(selector);
        e = entries[selector];
        if (e.impl == address(0)) revert EntryUnknown(selector);
    }

    /// @dev Look up a class, reverting with the right error for unknown / tombstoned.
    function _classOf(bytes4 classId) internal view returns (ClassMetadata memory cm) {
        if (tombstoned[classId]) revert ClassRemoved(classId);
        cm = classes[classId];
        if (cm.interfaceTag == bytes4(0)) revert ClassUnknown(classId);
    }

    /// @dev Resolve the requestor's signed `Requirements.selector` against the seal's
    ///      first-4-byte selector. The signed bytes4 carries one of three meanings:
    ///        * `0x00000000`   — chain default; the seal's entry must belong to the
    ///                           class flagged `isDefault == true`.
    ///        * a class id     — the seal's entry must belong to exactly that class.
    ///        * an entry id    — the seal's selector must equal the signed value.
    ///      Reverts with a per-meaning error so the failure mode is unambiguous in
    ///      tests and traces.
    function _matchSignedSelector(bytes4 sealSel, bytes4 signedSel, bytes4 sealClassId) internal view {
        if (signedSel == CHAIN_DEFAULT_SENTINEL) {
            bytes4 def = defaultClassId;
            if (def == bytes4(0)) revert NoDefaultClass();
            if (sealClassId != def) revert SignedDefaultClassMismatch(sealClassId, def);
            return;
        }
        if (tombstoned[signedSel]) {
            // A request signed against a now-tombstoned bytes4. We can't tell which
            // namespace it was in (both share `tombstoned`), so the diagnostic error is
            // unified: the protocol refuses to service it; brokers must drop such orders.
            revert SignedSelectorTombstoned(signedSel);
        }
        if (classes[signedSel].interfaceTag != bytes4(0)) {
            // Signed a class id — any entry under that class is acceptable.
            if (sealClassId != signedSel) revert SignedClassMismatch(signedSel, sealClassId);
            return;
        }
        if (entries[signedSel].impl != address(0)) {
            // Signed a specific entry selector — the seal must match exactly.
            if (sealSel != signedSel) revert SignedEntryMismatch(signedSel, sealSel);
            return;
        }
        // Signed bytes4 resolves to nothing — never registered, not tombstoned.
        revert SignedSelectorUnknown(signedSel);
    }

    /// @dev Tag predicates: keep the `interfaceTag == type(I).interfaceId` comparisons
    ///      out of the dispatch sites so the engine reads as plain English. These compile
    ///      to a single bytes4 equality and cost nothing at runtime.
    function _isVerifierTag(bytes4 tag) internal pure returns (bool) {
        return tag == type(IBoundlessVerifier).interfaceId;
    }

    function _isJointTag(bytes4 tag) internal pure returns (bool) {
        return tag == type(IBoundlessJointVerifierAssessor).interfaceId;
    }

    function _isAssessorTag(bytes4 tag) internal pure returns (bool) {
        return tag == type(IBoundlessAssessor).interfaceId;
    }

    /// @dev ERC-165 conformance check. We swallow reverts and treat them as "not
    ///      supported" so non-introspecting impls fail cleanly with `Erc165CheckFailed`.
    function _supportsInterface(address impl, bytes4 interfaceId) internal view returns (bool) {
        try IERC165(impl).supportsInterface(interfaceId) returns (bool ok) {
            return ok;
        } catch {
            return false;
        }
    }
}
