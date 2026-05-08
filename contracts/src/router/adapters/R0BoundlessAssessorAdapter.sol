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
import {RequestId} from "../../types/RequestId.sol";
import {Selector} from "../../types/Selector.sol";
import {MerkleProofish} from "../../libraries/MerkleProofish.sol";

/// @title R0BoundlessAssessorAdapter — `IBoundlessAssessor` adapter wrapping the
///        existing R0 STARK assessor verifier.
///
/// @notice Reconstructs the assessor journal digest verbatim from today's market
///         path and forwards to `IRiscZeroVerifier.verify(seal, ASSESSOR_IMAGE_ID,
///         journalDigest)`. Today's R0 STARK assessor proofs remain bit-identically
///         verifiable through the router — no guest changes required.
///
///         The `IBoundlessAssessor.verifyAssessor` interface intentionally surfaces
///         only `(requestDigests, claimDigests, seal)` — that's the universal seam.
///         Fields the existing R0 STARK journal also commits to (per-fill `id` and
///         `fulfillmentDataDigest`; per-batch `callbacks`, `selectors`, `prover`)
///         are specific to this adapter's binding shape, so they ride inside the
///         seal as an envelope:
///
///             bytes4 selector || abi.encode(Envelope) — where `Envelope.innerSeal`
///             is the underlying R0 STARK seal.
///
///         **Pinned at deploy time, immutable.** One adapter instance per
///         `(image id, underlying verifier, envelope shape)` triple. The image
///         id is set in the constructor and never changes. The adapter has no
///         governance role, no upgrade path, no mutable state.
///
///         **Rotation is router-level, not adapter-level.** When the assessor
///         guest image is updated, the operational pattern is:
///           1. Deploy a new `R0BoundlessAssessorAdapter` pinned to the new
///              image.
///           2. Governance `instantiate`s a new selector under `R0_ASSESSOR`
///              pointing at the new adapter.
///           3. Both selectors run in parallel. Brokers using the old image
///              select the old selector; brokers using the new image select
///              the new selector. The choice is broker-side and not visible to
///              requestors (the assessor selector is not requestor-signed).
///           4. Once all brokers have migrated and drained their queues of
///              old-image proofs, governance calls `removeEntry(oldSelector)`
///              to tombstone the old adapter — the same mechanism as today's
///              `DEPRECATED_ASSESSOR_EXPIRES_AT` deadline, but managed
///              manually via governance rather than by an in-contract
///              timestamp.
///
///         The same pattern applies to envelope/journal *shape* changes (a new
///         leaf field, a different journal binding) — those also require a new
///         adapter contract because the decode logic differs. So image rotation
///         and envelope-shape rotation share one operational ceremony.
///
///         The underlying verifier is pinned at deploy time (today:
///         `RiscZeroSetVerifier`, since the broker produces set-inclusion seals
///         for the assessor) — never the existing R0 router. Every selector
///         reachable through BoundlessRouter is explicit at the top level, with
///         no transitive trust of the upstream R0 router's selector set.
///
/// @dev    TODO: once the market takes `ProofRequest[]` at fulfill time and
///         re-verifies each request's EIP-712 digest against the lock, the
///         journal's per-batch `callbacks` and `selectors` fields become
///         redundant — the market sources them directly from the verified
///         request struct, so a malicious broker can no longer lie about
///         them. The same applies to the per-fill `id` in the envelope (also
///         bound by `requestDigest`). At the next assessor image rotation the
///         guest can drop those commitments, and the corresponding adapter
///         version (a fresh contract under a new `R0_ASSESSOR` selector, per
///         the rotation pattern above) shrinks the envelope and the journal
///         binding accordingly. Until then this adapter keeps reconstructing
///         the existing shape verbatim — the redundancy is harmless, just
///         calldata waste.
contract R0BoundlessAssessorAdapter is IBoundlessAssessor, IERC165 {
    /// @notice Off-chain envelope packing the journal extras the universal
    ///         `IBoundlessAssessor` interface doesn't surface, plus the
    ///         underlying R0 STARK seal. The image id is implicit (pinned by
    ///         the adapter's immutable `ASSESSOR_IMAGE_ID`); the prover is
    ///         passed as a universal arg, not via the envelope.
    struct Envelope {
        /// @notice Per-fill `RequestId`. Length must equal `requestDigests.length`.
        RequestId[] ids;
        /// @notice Per-fill fulfillment-data digest. Length must equal `requestDigests.length`.
        bytes32[] fulfillmentDataDigests;
        /// @notice Optional callbacks committed in the journal.
        AssessorCallback[] callbacks;
        /// @notice Optional per-fill selectors committed in the journal.
        Selector[] selectors;
        /// @notice The R0 STARK seal that the underlying verifier consumes.
        bytes innerSeal;
    }

    /// @notice The specific R0 verifier this adapter forwards to. Pinned at
    ///         deploy time — never the existing R0 router.
    IRiscZeroVerifier public immutable RISC_ZERO_VERIFIER;

    /// @notice The assessor guest image id this adapter binds to. Pinned at
    ///         deploy time. Rotation is via a fresh adapter deployment plus
    ///         BoundlessRouter selector update (see contract NatSpec).
    bytes32 public immutable ASSESSOR_IMAGE_ID;

    error MalformedEnvelope();
    error EnvelopeLengthMismatch();

    constructor(IRiscZeroVerifier riscZeroVerifier, bytes32 assessorImageId) {
        require(address(riscZeroVerifier) != address(0), "R0BoundlessAssessorAdapter: zero verifier");
        require(assessorImageId != bytes32(0), "R0BoundlessAssessorAdapter: zero image id");
        RISC_ZERO_VERIFIER = riscZeroVerifier;
        ASSESSOR_IMAGE_ID = assessorImageId;
    }

    /// @inheritdoc IBoundlessAssessor
    function verifyAssessor(
        bytes32[] calldata requestDigests,
        bytes32[] calldata claimDigests,
        address prover,
        bytes calldata assessorSeal
    ) external view {
        // Strip the router's 4-byte selector prefix; the rest is the ABI-encoded envelope.
        if (assessorSeal.length < 4) revert MalformedEnvelope();
        Envelope memory env = abi.decode(assessorSeal[4:], (Envelope));

        uint256 n = requestDigests.length;
        if (claimDigests.length != n || env.ids.length != n || env.fulfillmentDataDigests.length != n) {
            revert EnvelopeLengthMismatch();
        }

        // Reconstruct the merkle leaves the assessor guest committed to.
        bytes32[] memory leaves = new bytes32[](n);
        for (uint256 i = 0; i < n; i++) {
            leaves[i] = AssessorCommitment({
                    index: i,
                    id: env.ids[i],
                    requestDigest: requestDigests[i],
                    claimDigest: claimDigests[i],
                    fulfillmentDataDigest: env.fulfillmentDataDigests[i]
                }).eip712Digest();
        }
        bytes32 batchRoot = MerkleProofish.processTree(leaves);

        // Reconstruct the journal binding identically to today's market path. The
        // `prover` arg is committed by the journal — the R0 STARK fails if the seal
        // was produced against a different prover than the one passed by the caller.
        bytes32 journalDigest = sha256(
            abi.encode(
                AssessorJournal({root: batchRoot, callbacks: env.callbacks, selectors: env.selectors, prover: prover})
            )
        );

        RISC_ZERO_VERIFIER.verify(env.innerSeal, ASSESSOR_IMAGE_ID, journalDigest);
    }

    /// @inheritdoc IERC165
    function supportsInterface(bytes4 interfaceId) external pure returns (bool) {
        return interfaceId == type(IBoundlessAssessor).interfaceId || interfaceId == type(IERC165).interfaceId;
    }
}
