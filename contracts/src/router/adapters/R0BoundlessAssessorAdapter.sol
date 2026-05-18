// Copyright 2026 Boundless Foundation, Inc.
//
// Use of this source code is governed by the Business Source License
// as found in the LICENSE-BSL file.
// SPDX-License-Identifier: BUSL-1.1

pragma solidity ^0.8.26;

import {IERC165} from "@openzeppelin/contracts/utils/introspection/IERC165.sol";
import {IRiscZeroVerifier} from "risc0/IRiscZeroVerifier.sol";

import {IBoundlessAssessor} from "../interfaces/IBoundlessAssessor.sol";
import {AssessorCallback} from "../../types/AssessorCallback.sol";
import {AssessorCommitment} from "../../types/AssessorCommitment.sol";
import {AssessorJournal} from "../../types/AssessorJournal.sol";
import {Fulfillment, FulfillmentLibrary} from "../../types/Fulfillment.sol";
import {Selector} from "../../types/Selector.sol";
import {SlimRequest} from "../../types/SlimRequest.sol";
import {MerkleProofish} from "../../libraries/MerkleProofish.sol";

/// @title R0BoundlessAssessorAdapter — `IBoundlessAssessor` adapter wrapping the
///        existing R0 STARK assessor verifier.
///
/// @notice Reconstructs the existing R0 STARK assessor journal from the trusted
///         slim payload and forwards to `IRiscZeroVerifier.verify(seal,
///         ASSESSOR_IMAGE_ID, journalDigest)`. Today's R0 STARK assessor proofs
///         remain bit-identically verifiable through the router — no guest
///         changes required.
///
///         The `IBoundlessAssessor.verifyAssessor` interface surfaces the
///         universal seam: `(SlimRequest[], Fulfillment[], requestDigests[],
///         prover, seal)`. The market has already bound each `SlimRequest`
///         to a client-signed `requestDigest` via the lock or transient
///         `FulfillmentContext`, so this adapter trusts the payload and
///         derives every journal field from it:
///           * Per-fill `id`, `callback`, `selector` — read from `SlimRequest`.
///           * Per-fill `fulfillmentDataDigest` — `FulfillmentLibrary.fulfillmentDataDigest`
///             over `fills[i].{fulfillmentDataType, fulfillmentData}`.
///           * Per-fill merkle leaf — `AssessorCommitment{i, id, requestDigest,
///             claimDigest, fulfillmentDataDigest}.eip712Digest()`.
///           * Per-batch `callbacks[]` / `selectors[]` — sparse arrays built
///             from non-zero `SlimRequest.callback.addr` / `selector` entries.
///           * Per-batch `prover` — universal arg.
///         The only adapter-specific bytes in the seal are the underlying R0
///         STARK proof; everything else lives in `SlimRequest` / `Fulfillment`.
///
///         **Pinned at deploy time, immutable.** One adapter instance per
///         `(image id, underlying verifier)` pair. The image id is set in
///         the constructor and never changes. No governance role, no upgrade
///         path, no mutable state.
///
///         **Rotation is router-level, not adapter-level.** When the assessor
///         guest image is updated, the operational pattern is:
///           1. Deploy a new `R0BoundlessAssessorAdapter` pinned to the new image.
///           2. Governance `instantiate`s a new selector under `R0_ASSESSOR`
///              pointing at the new adapter.
///           3. Both selectors run in parallel. Brokers using the old image
///              select the old selector; brokers using the new image select
///              the new one. The choice is broker-side and not visible to
///              requestors (the assessor selector is not requestor-signed).
///           4. Once all brokers have migrated and drained their queues of
///              old-image proofs, governance calls `removeEntry(oldSelector)`
///              to tombstone the old adapter.
///
///         The underlying verifier is pinned at deploy time (today:
///         `RiscZeroSetVerifier`, since the broker produces set-inclusion seals
///         for the assessor) — never the existing R0 router. Every selector
///         reachable through `BoundlessRouter` is explicit at the top level,
///         with no transitive trust of the upstream R0 router's selector set.
///
/// @dev    The existing R0 assessor guest commits to per-fill `id`,
///         `fulfillmentDataDigest`, and per-batch `callbacks` / `selectors` in
///         its journal — fields that are also now bound by the market's
///         `_verifyBinding` (via the `requestDigest` reconstruction from the
///         slim payload). The guest's commitment is therefore redundant with
///         the market's. At the next assessor-image rotation, the guest can
///         drop those journal fields and a fresh adapter version (deployed
///         under a new `R0_ASSESSOR` selector, per the rotation pattern above)
///         would shrink the journal binding accordingly. Until that rotation,
///         this adapter reconstructs the existing journal shape verbatim — the
///         redundancy costs a few keccak hashes per fill but is otherwise
///         harmless.
contract R0BoundlessAssessorAdapter is IBoundlessAssessor, IERC165 {
    /// @notice The specific R0 verifier this adapter forwards to. Pinned at
    ///         deploy time — never the existing R0 router.
    IRiscZeroVerifier public immutable RISC_ZERO_VERIFIER;

    /// @notice The assessor guest image id this adapter binds to. Pinned at
    ///         deploy time. Rotation is via a fresh adapter deployment plus
    ///         BoundlessRouter selector update (see contract NatSpec).
    bytes32 public immutable ASSESSOR_IMAGE_ID;

    error MalformedSeal();
    error LengthMismatch();

    constructor(IRiscZeroVerifier riscZeroVerifier, bytes32 assessorImageId) {
        require(address(riscZeroVerifier) != address(0), "R0BoundlessAssessorAdapter: zero verifier");
        require(assessorImageId != bytes32(0), "R0BoundlessAssessorAdapter: zero image id");
        RISC_ZERO_VERIFIER = riscZeroVerifier;
        ASSESSOR_IMAGE_ID = assessorImageId;
    }

    /// @inheritdoc IBoundlessAssessor
    function verifyAssessor(
        SlimRequest[] calldata requests,
        Fulfillment[] calldata fills,
        bytes32[] calldata requestDigests,
        address prover,
        bytes calldata assessorSeal
    ) external view {
        uint256 n = requests.length;
        if (fills.length != n || requestDigests.length != n) revert LengthMismatch();

        // Strip the router's 4-byte selector prefix; the rest is the inner STARK seal.
        if (assessorSeal.length < 4) revert MalformedSeal();
        bytes calldata innerSeal = assessorSeal[4:];

        // Count sparse callback / selector entries so we can size memory arrays
        // exactly (Solidity memory arrays can't grow dynamically).
        uint256 cbCount;
        uint256 selCount;
        for (uint256 i = 0; i < n; i++) {
            if (requests[i].callback.addr != address(0)) cbCount++;
            if (requests[i].selector != bytes4(0)) selCount++;
        }
        AssessorCallback[] memory callbacks = new AssessorCallback[](cbCount);
        Selector[] memory selectors = new Selector[](selCount);

        // Reconstruct merkle leaves and populate sparse arrays in one pass.
        bytes32[] memory leaves = new bytes32[](n);
        uint256 cbIdx;
        uint256 selIdx;
        for (uint256 i = 0; i < n; i++) {
            bytes32 fulfillmentDataDigest =
                FulfillmentLibrary.fulfillmentDataDigest(fills[i].fulfillmentDataType, fills[i].fulfillmentData);
            leaves[i] = AssessorCommitment({
                index: i,
                id: requests[i].id,
                requestDigest: requestDigests[i],
                claimDigest: fills[i].claimDigest,
                fulfillmentDataDigest: fulfillmentDataDigest
            }).eip712Digest();

            if (requests[i].callback.addr != address(0)) {
                callbacks[cbIdx++] = AssessorCallback({
                    index: uint16(i),
                    addr: requests[i].callback.addr,
                    gasLimit: requests[i].callback.gasLimit
                });
            }
            if (requests[i].selector != bytes4(0)) {
                selectors[selIdx++] = Selector({index: uint16(i), value: requests[i].selector});
            }
        }

        bytes32 batchRoot = MerkleProofish.processTree(leaves);

        // Reconstruct the journal binding identically to today's market path. The
        // `prover` arg is committed by the journal — the R0 STARK fails if the seal
        // was produced against a different prover than the one passed by the caller.
        bytes32 journalDigest = sha256(
            abi.encode(AssessorJournal({root: batchRoot, callbacks: callbacks, selectors: selectors, prover: prover}))
        );

        RISC_ZERO_VERIFIER.verify(innerSeal, ASSESSOR_IMAGE_ID, journalDigest);
    }

    /// @inheritdoc IERC165
    function supportsInterface(bytes4 interfaceId) external pure returns (bool) {
        return interfaceId == type(IBoundlessAssessor).interfaceId || interfaceId == type(IERC165).interfaceId;
    }
}
