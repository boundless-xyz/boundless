// Copyright 2026 Boundless Foundation, Inc.
//
// Use of this source code is governed by the Business Source License
// as found in the LICENSE-BSL file.
// SPDX-License-Identifier: BUSL-1.1

pragma solidity ^0.8.26;

import {BoundlessRouter} from "../src/router/BoundlessRouter.sol";
import {IBoundlessVerifier} from "../src/router/interfaces/IBoundlessVerifier.sol";
import {IBoundlessAssessor} from "../src/router/interfaces/IBoundlessAssessor.sol";

/// @notice Canonical Boundless router configuration: class ids, class metadata, and the
///         release-coupled selectors. Single source for the deploy / bootstrap / manage
///         scripts; mirrored by the class-id constants in the `boundless-market` Rust crate.
///
///         A class is a proof-type version family; its entries are versions of that proof
///         type. Requestors sign one of three selector forms: the `0x00000000` sentinel
///         (chain default), a class id ("any version of this proof type"), or an entry
///         selector (exact version pin).
library RouterConfig {
    /// @notice Set-inclusion proofs against the aggregated batch root. The chain-default
    ///         class: sentinel-signed requests are fulfilled with aggregated seals.
    bytes4 internal constant R0_SET_INCLUSION_CLASS_ID = bytes4(0xAA000001);
    /// @notice Assessor class required by every verifier class below. Holds the R0 STARK
    ///         assessor and the native on-chain assessor.
    bytes4 internal constant R0_ASSESSOR_CLASS_ID = bytes4(0xAA000002);
    /// @notice Per-fill R0 groth16 root proofs.
    bytes4 internal constant R0_GROTH16_CLASS_ID = bytes4(0xAA000003);
    /// @notice Per-fill R0 blake3-groth16 root proofs.
    bytes4 internal constant R0_GROTH16_BLAKE3_CLASS_ID = bytes4(0xAA000004);

    /// @notice Release-coupled selectors of the current risc0-ethereum verifiers. The
    ///         bootstrap verifies each against the upstream `RiscZeroVerifierRouter`
    ///         before registering and skips selectors the chain does not serve (localnet
    ///         dev verifiers use dynamic selectors and never match). The set-inclusion
    ///         selector is deliberately absent here: it derives from the set-builder
    ///         guest image id and is read from the set verifier's `SELECTOR()` instead.
    bytes4 internal constant GROTH16_SELECTOR = bytes4(0x73c457ba);
    bytes4 internal constant GROTH16_BLAKE3_SELECTOR = bytes4(0x62f049f6);

    /// @notice Stable protocol selector of the native on-chain assessor entry.
    bytes4 internal constant ONCHAIN_ASSESSOR_SELECTOR = bytes4(0x00000022);
    /// @notice Guest-version-coupled selector of the R0 STARK assessor entry.
    bytes4 internal constant R0_ASSESSOR_SELECTOR = bytes4(0x00000024);

    /// @notice Per-call gas cap for verifier entries. Must cover the most expensive
    ///         curated verifier plus adapter overhead: a real groth16 verification costs
    ///         ~250k gas (blake3-groth16 slightly more). The cap bounds what a runaway
    ///         adapter can burn per fill, not the expected cost.
    uint64 internal constant VERIFIER_CLASS_GAS_LIMIT = 500_000;
    /// @notice Per-call gas cap for assessor entries.
    uint64 internal constant ASSESSOR_CLASS_GAS_LIMIT = 500_000;

    function assessorClass() internal pure returns (BoundlessRouter.ClassMetadata memory) {
        return BoundlessRouter.ClassMetadata({
            interfaceTag: type(IBoundlessAssessor).interfaceId,
            permissionlessInstantiate: false,
            isDefault: false,
            requiredAssessorClass: bytes4(0),
            schemaArtifact: bytes32(0),
            schemaArtifactUrl: "",
            defaultGasLimit: ASSESSOR_CLASS_GAS_LIMIT,
            label: "R0Assessor"
        });
    }

    function setInclusionClass() internal pure returns (BoundlessRouter.ClassMetadata memory) {
        return _verifierClass("R0SetInclusion", true);
    }

    function groth16Class() internal pure returns (BoundlessRouter.ClassMetadata memory) {
        return _verifierClass("R0Groth16", false);
    }

    function groth16Blake3Class() internal pure returns (BoundlessRouter.ClassMetadata memory) {
        return _verifierClass("R0Groth16Blake3", false);
    }

    function _verifierClass(string memory label, bool isDefault)
        private
        pure
        returns (BoundlessRouter.ClassMetadata memory)
    {
        return BoundlessRouter.ClassMetadata({
            interfaceTag: type(IBoundlessVerifier).interfaceId,
            permissionlessInstantiate: false,
            isDefault: isDefault,
            requiredAssessorClass: R0_ASSESSOR_CLASS_ID,
            schemaArtifact: bytes32(0),
            schemaArtifactUrl: "",
            defaultGasLimit: VERIFIER_CLASS_GAS_LIMIT,
            label: label
        });
    }
}
