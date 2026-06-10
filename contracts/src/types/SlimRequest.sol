// Copyright 2026 Boundless Foundation, Inc.
//
// Use of this source code is governed by the Business Source License
// as found in the LICENSE-BSL file.
// SPDX-License-Identifier: BUSL-1.1

pragma solidity ^0.8.26;

import {RequestId} from "./RequestId.sol";
import {Predicate, PredicateLibrary} from "./Predicate.sol";
import {Callback, CallbackLibrary} from "./Callback.sol";
import {RequirementsLibrary} from "./Requirements.sol";
import {ProofRequestLibrary} from "./ProofRequest.sol";

using SlimRequestLibrary for SlimRequest global;

/// @title SlimRequest — minimal per-fill payload bound to a signed `ProofRequest`.
///
/// @notice The market needs the actual values of the fields it will act on
/// (predicate for assessor evaluation, callback for dispatch, selector for
/// router enforcement) and only the digests of fields it never reads at fulfill
/// time (imageUrl, input, offer). `SlimRequest` carries the former in full and
/// the latter as pre-computed digests, so the market can reconstruct the EIP-712
/// `requestDigest` and assert it matches the value stored at lock time.
///
/// @dev Reconstruction mirrors `ProofRequest.eip712Digest()` exactly. The prover
/// (off-chain) pre-computes `imageUrlHash`, `inputDigest`, and `offerDigest` from
/// the original `ProofRequest`. The struct hash is what `reconstructRequestDigest`
/// returns; the market wraps it with `_hashTypedDataV4` before comparing to the
/// domain-bound value stored at lock time (or written to `FulfillmentContext` by
/// `priceRequest`). Once this assertion passes, every field of `SlimRequest` is
/// bound to the client's signed request, so downstream consumers (assessor
/// adapter, callback dispatch) can trust the payload without re-verification.
///
/// The market verifies the binding as:
///
/// ```text
/// structHash = hash(
///     PROOF_REQUEST_TYPEHASH,
///     slim.id,
///     hash(REQ_TYPEHASH,
///          hash(CB_TYPEHASH, callback.addr, callback.gasLimit),
///          hash(PRED_TYPEHASH, predicate.type, keccak256(predicate.data)),
///          slim.selector),
///     slim.imageUrlHash,
///     slim.inputDigest,
///     slim.offerDigest
/// )
/// requestDigest = _hashTypedDataV4(structHash)
/// assert requestDigest == requestLocks[slim.id].requestDigest;
/// ```
struct SlimRequest {
    /// @notice Request identifier (client address + 32-bit index).
    RequestId id;
    /// @notice The predicate the assessor will evaluate.
    Predicate predicate;
    /// @notice Callback configuration (address(0) ⇒ no callback).
    Callback callback;
    /// @notice The requestor's signed verifier selector.
    bytes4 selector;
    /// @notice `keccak256(bytes(imageUrl))`. Pre-computed by the prover.
    bytes32 imageUrlHash;
    /// @notice `Input.eip712Digest()`. Pre-computed by the prover.
    bytes32 inputDigest;
    /// @notice `Offer.eip712Digest()`. Pre-computed by the prover.
    bytes32 offerDigest;
}

library SlimRequestLibrary {
    /// @notice Reconstruct the EIP-712 struct hash of the original
    ///         `ProofRequest` from a `SlimRequest`.
    /// @dev    Must produce a byte-identical result to
    ///         `ProofRequestLibrary.eip712Digest(ProofRequest)` when the slim
    ///         fields are derived from a real `ProofRequest`. The caller is
    ///         responsible for domain-binding via `_hashTypedDataV4` when a
    ///         `requestDigest` comparable to the market's lock storage is
    ///         needed.
    function reconstructRequestDigest(SlimRequest memory slim) internal pure returns (bytes32) {
        bytes32 callbackDigest = CallbackLibrary.eip712Digest(slim.callback);
        bytes32 predicateDigest = PredicateLibrary.eip712Digest(slim.predicate);
        bytes32 requirementsDigest = keccak256(
            abi.encode(RequirementsLibrary.REQUIREMENTS_TYPEHASH, callbackDigest, predicateDigest, slim.selector)
        );
        return keccak256(
            abi.encode(
                ProofRequestLibrary.PROOF_REQUEST_TYPEHASH,
                slim.id,
                requirementsDigest,
                slim.imageUrlHash,
                slim.inputDigest,
                slim.offerDigest
            )
        );
    }
}
