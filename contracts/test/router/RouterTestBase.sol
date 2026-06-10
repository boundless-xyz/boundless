// Copyright 2026 Boundless Foundation, Inc.
//
// Use of this source code is governed by the Business Source License
// as found in the LICENSE-BSL file.
// SPDX-License-Identifier: BUSL-1.1

pragma solidity ^0.8.26;

import {Test} from "forge-std/Test.sol";
import {UnsafeUpgrades} from "openzeppelin-foundry-upgrades/Upgrades.sol";

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

/// @title RouterTestBase — minimal shared setup for `BoundlessRouter` unit tests.
///
/// @notice Deploys an EMPTY router (no classes, no entries) via UUPS proxy.
///         Every test builds exactly the state it needs via the helper
///         methods below. This is intentionally lighter than `BenchBase`,
///         which pre-registers an adapter ecosystem and fights tests that
///         want to exercise registry primitives.
abstract contract RouterTestBase is Test {
    BoundlessRouter internal router;

    address internal constant ADMIN = address(0xA11CE);
    address internal constant USER = address(0xB0B);

    function setUp() public virtual {
        BoundlessRouter implementation = new BoundlessRouter();
        address proxy = UnsafeUpgrades.deployUUPSProxy(
            address(implementation), abi.encodeCall(BoundlessRouter.initialize, (ADMIN))
        );
        router = BoundlessRouter(proxy);
    }

    // ─── Class / entry helpers ────────────────────────────────────────────

    function _addVerifierClass(bytes4 classId, bytes4 assessorClassId, bool isDefault, bool permissionless) internal {
        vm.prank(ADMIN);
        router.addClass(
            classId,
            BoundlessRouter.ClassMetadata({
                interfaceTag: type(IBoundlessVerifier).interfaceId,
                permissionlessInstantiate: permissionless,
                isDefault: isDefault,
                requiredAssessorClass: assessorClassId,
                schemaArtifact: bytes32(0),
                schemaArtifactUrl: "",
                defaultGasLimit: 100_000,
                label: ""
            })
        );
    }

    function _addAssessorClass(bytes4 classId, bool permissionless) internal {
        vm.prank(ADMIN);
        router.addClass(
            classId,
            BoundlessRouter.ClassMetadata({
                interfaceTag: type(IBoundlessAssessor).interfaceId,
                permissionlessInstantiate: permissionless,
                isDefault: false,
                requiredAssessorClass: bytes4(0),
                schemaArtifact: bytes32(0),
                schemaArtifactUrl: "",
                defaultGasLimit: 10_000_000,
                label: ""
            })
        );
    }

    function _addJointClass(bytes4 classId, bool permissionless) internal {
        vm.prank(ADMIN);
        router.addClass(
            classId,
            BoundlessRouter.ClassMetadata({
                interfaceTag: type(IBoundlessJointVerifierAssessor).interfaceId,
                permissionlessInstantiate: permissionless,
                isDefault: false,
                requiredAssessorClass: bytes4(0),
                schemaArtifact: bytes32(0),
                schemaArtifactUrl: "",
                defaultGasLimit: 100_000,
                label: ""
            })
        );
    }

    function _instantiateAs(address caller, bytes4 selector, address impl, bytes4 classId, uint64 gasLimit) internal {
        vm.prank(caller);
        router.instantiate(selector, impl, classId, gasLimit);
    }

    function _instantiateAsAdmin(bytes4 selector, address impl, bytes4 classId) internal {
        _instantiateAs(ADMIN, selector, impl, classId, 0);
    }

    // ─── Batch / seal helpers ─────────────────────────────────────────────

    /// @dev Returns `selector || hex"deadbeef"` — minimum viable seal that
    ///      passes `_sealSelector`. Tests that care about the seal contents
    ///      should build their own bytes.
    function _seal(bytes4 selector) internal pure returns (bytes memory) {
        return abi.encodePacked(selector, hex"deadbeef");
    }

    /// @dev Returns `selector || tail`. Tail can be empty (4-byte seal).
    function _seal(bytes4 selector, bytes memory tail) internal pure returns (bytes memory) {
        return abi.encodePacked(selector, tail);
    }

    /// @dev Single-fill `FulfillmentBatch` with a zeroed slim payload. Most
    ///      router unit tests don't care about the slim contents — they
    ///      drive the dispatch tree via the seal selector and the signed
    ///      selector. Tests that need realistic payloads build them inline.
    function _minimalBatch(bytes4 sealSel, bytes4 signedSel, address prover, bytes memory assessorSeal)
        internal
        pure
        returns (FulfillmentBatch memory batch)
    {
        SlimRequest[] memory requests = new SlimRequest[](1);
        requests[0] = SlimRequest({
            id: RequestId.wrap(0),
            predicate: Predicate({predicateType: PredicateType.ClaimDigestMatch, data: bytes("")}),
            callback: Callback({addr: address(0), gasLimit: 0}),
            selector: signedSel,
            imageUrlHash: bytes32(0),
            inputDigest: bytes32(0),
            offerDigest: bytes32(0)
        });

        Fulfillment[] memory fills = new Fulfillment[](1);
        fills[0] = Fulfillment({
            claimDigest: bytes32(0),
            fulfillmentDataType: FulfillmentDataType.None,
            fulfillmentData: bytes(""),
            seal: abi.encodePacked(sealSel, hex"deadbeef")
        });

        batch = FulfillmentBatch({requests: requests, fills: fills, assessorSeal: assessorSeal, prover: prover});
    }

    /// @dev Wraps a single seal into the `_minimalBatch` shape, useful for
    ///      tests that want explicit control over the per-fill seal bytes.
    function _minimalBatchWithSeal(
        bytes memory perFillSeal,
        bytes4 signedSel,
        address prover,
        bytes memory assessorSeal
    ) internal pure returns (FulfillmentBatch memory batch) {
        SlimRequest[] memory requests = new SlimRequest[](1);
        requests[0] = SlimRequest({
            id: RequestId.wrap(0),
            predicate: Predicate({predicateType: PredicateType.ClaimDigestMatch, data: bytes("")}),
            callback: Callback({addr: address(0), gasLimit: 0}),
            selector: signedSel,
            imageUrlHash: bytes32(0),
            inputDigest: bytes32(0),
            offerDigest: bytes32(0)
        });

        Fulfillment[] memory fills = new Fulfillment[](1);
        fills[0] = Fulfillment({
            claimDigest: bytes32(0),
            fulfillmentDataType: FulfillmentDataType.None,
            fulfillmentData: bytes(""),
            seal: perFillSeal
        });

        batch = FulfillmentBatch({requests: requests, fills: fills, assessorSeal: assessorSeal, prover: prover});
    }

    /// @dev Allocate a zeroed `bytes32[]` of length `n` matching a batch's fill count.
    function _emptyDigests(uint256 n) internal pure returns (bytes32[] memory) {
        return new bytes32[](n);
    }
}
