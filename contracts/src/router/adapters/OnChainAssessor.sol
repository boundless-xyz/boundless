// Copyright 2026 Boundless Foundation, Inc.
//
// Use of this source code is governed by the Business Source License
// as found in the LICENSE-BSL file.
// SPDX-License-Identifier: BUSL-1.1

pragma solidity ^0.8.26;

import {IERC165} from "@openzeppelin/contracts/utils/introspection/IERC165.sol";
import {ECDSA} from "@openzeppelin/contracts/utils/cryptography/ECDSA.sol";
import {ReceiptClaim, ReceiptClaimLib} from "risc0/IRiscZeroVerifier.sol";

import {IBoundlessAssessor} from "../interfaces/IBoundlessAssessor.sol";
import {SlimRequest} from "../../types/SlimRequest.sol";
import {Fulfillment} from "../../types/Fulfillment.sol";
import {FulfillmentDataLibrary, FulfillmentDataType} from "../../types/FulfillmentData.sol";
import {PredicateType} from "../../types/Predicate.sol";

/// @title OnChainAssessor — native Solidity fulfillment-check adapter.
///
/// @notice Implements `IBoundlessAssessor` by evaluating each fill's predicate
///         directly on-chain. No zkVM, no merkle tree, no STARK proof. The
///         market has already bound `SlimRequest` to a signed lock before
///         dispatch, so the adapter trusts the supplied predicate.
///
///         Per-fill checks:
///           1. Predicate satisfaction:
///                * `ClaimDigestMatch` — `predicate.data == fill.claimDigest`.
///                * `DigestMatch` / `PrefixMatch` — decode `(imageId, journal)`
///                  from `fill.fulfillmentData` and run `PredicateLibrary.eval`.
///           2. Claim-digest binding: the supplied `(imageId, journal)` must
///              reconstruct to `fill.claimDigest` via
///              `ReceiptClaimLib.ok(imageId, sha256(abi.encode(journal))).digest()`.
///              Without this, a malicious prover could submit a valid seal for
///              one computation and journal bytes from a different one.
///
///         Per batch:
///           3. Prover binding: `assessorSeal` carries an ECDSA signature by
///              `prover` over the EIP-712 hash of `(prover, requestDigests[],
///              claimDigests[])`. The adapter recovers the signer and asserts
///              it equals `prover`. This is the on-chain equivalent of the
///              R0 STARK adapter's journal commitment to `prover`.
///
///         Stateless and immutable; no governance role, no upgrade path.
contract OnChainAssessor is IBoundlessAssessor, IERC165 {
    using ReceiptClaimLib for ReceiptClaim;

    /// @notice EIP-712 type for the fulfillment-batch authorization signed by `prover`.
    string internal constant FULFILLMENT_BATCH_AUTH_TYPE =
        "FulfillmentBatchAuth(address prover,bytes32[] requestDigests,bytes32[] claimDigests)";
    bytes32 internal constant FULFILLMENT_BATCH_AUTH_TYPEHASH = keccak256(bytes(FULFILLMENT_BATCH_AUTH_TYPE));

    /// @notice EIP-712 domain pinned at deploy time (chain id + verifying contract).
    bytes32 public immutable DOMAIN_SEPARATOR;

    /// @notice A fill's predicate evaluation returned false.
    error PredicateFailed(uint256 index);

    /// @notice `(imageId, journal)` does not reconstruct to `fill.claimDigest`.
    error ClaimDigestMismatch(uint256 index);

    /// @notice The predicate requires a journal but the fulfillment data type
    ///         indicates none was attached.
    error MissingFulfillmentData(uint256 index);

    /// @notice `requests.length` and `fills.length` must match.
    error LengthMismatch();

    /// @notice The prover signature was malformed (not exactly 65 bytes).
    error MalformedProverSignature();

    /// @notice The recovered signer does not equal `prover`.
    error ProverSignatureMismatch(address recovered, address expected);

    constructor() {
        DOMAIN_SEPARATOR = keccak256(
            abi.encode(
                keccak256("EIP712Domain(string name,string version,uint256 chainId,address verifyingContract)"),
                keccak256("OnChainAssessor"),
                keccak256("1"),
                block.chainid,
                address(this)
            )
        );
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

        // Per-fill: predicate satisfaction + claim-digest binding. Collect
        // claimDigests for the per-batch signature hash.
        bytes32[] memory claimDigests = new bytes32[](n);
        for (uint256 i = 0; i < n; i++) {
            PredicateType ptype = requests[i].predicate.predicateType;
            if (ptype == PredicateType.ClaimDigestMatch) {
                // Predicate.data == fill.claimDigest. This is itself the binding —
                // the predicate's claim digest IS the value the verifier proved.
                if (!requests[i].predicate.eval(fills[i].claimDigest)) {
                    revert PredicateFailed(i);
                }
            } else {
                if (fills[i].fulfillmentDataType != FulfillmentDataType.ImageIdAndJournal) {
                    revert MissingFulfillmentData(i);
                }
                (bytes32 imageId, bytes calldata journal) =
                    FulfillmentDataLibrary.decodePackedImageIdAndJournal(fills[i].fulfillmentData);

                // Predicate match: imageId + journal-prefix-or-digest matches what the client signed.
                if (!requests[i].predicate.eval(imageId, journal)) {
                    revert PredicateFailed(i);
                }
                // Claim-digest binding: the (imageId, journal) the prover supplied must
                // reconstruct to fill.claimDigest. Without this, the prover could submit
                // a valid seal for a different computation entirely.
                bytes32 reconstructed =
                    ReceiptClaimLib.ok(imageId, sha256(abi.encode(journal))).digest();
                if (reconstructed != fills[i].claimDigest) {
                    revert ClaimDigestMismatch(i);
                }
            }
            claimDigests[i] = fills[i].claimDigest;
        }

        // Per batch: prover signature over (prover, requestDigests, claimDigests).
        _verifyProverSignature(prover, requestDigests, claimDigests, assessorSeal);
    }

    /// @dev Recover the signer from `assessorSeal` (the bytes after the 4-byte
    ///      router selector prefix) and assert it equals `prover`.
    function _verifyProverSignature(
        address prover,
        bytes32[] memory requestDigests,
        bytes32[] memory claimDigests,
        bytes calldata assessorSeal
    ) internal view {
        // assessorSeal = 4-byte router selector || 65-byte ECDSA signature.
        if (assessorSeal.length != 4 + 65) revert MalformedProverSignature();
        bytes calldata signature = assessorSeal[4:];

        bytes32 structHash = keccak256(
            abi.encode(
                FULFILLMENT_BATCH_AUTH_TYPEHASH,
                prover,
                keccak256(abi.encodePacked(requestDigests)),
                keccak256(abi.encodePacked(claimDigests))
            )
        );
        bytes32 digest = keccak256(abi.encodePacked("\x19\x01", DOMAIN_SEPARATOR, structHash));
        address recovered = ECDSA.recover(digest, signature);
        if (recovered != prover) revert ProverSignatureMismatch(recovered, prover);
    }

    /// @inheritdoc IERC165
    function supportsInterface(bytes4 interfaceId) external pure returns (bool) {
        return interfaceId == type(IBoundlessAssessor).interfaceId || interfaceId == type(IERC165).interfaceId;
    }
}
