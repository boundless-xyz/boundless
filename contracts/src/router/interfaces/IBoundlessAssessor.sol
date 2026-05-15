// Copyright 2026 Boundless Foundation, Inc.
//
// Use of this source code is governed by the Business Source License
// as found in the LICENSE-BSL file.
// SPDX-License-Identifier: BUSL-1.1

pragma solidity ^0.8.26;

import {SlimRequest} from "../../types/SlimRequest.sol";
import {Fulfillment} from "../../types/Fulfillment.sol";

/// @title IBoundlessAssessor — per-batch fulfillment-check seam.
///
/// @notice An adapter implementing this interface vouches, for each fill in a
///         batch, that the fulfillment satisfies the requestor's
///         `predicate`. The adapter does NOT verify request authenticity —
///         that is the market's job (binding check before dispatch).
///
///         Two adapters are expected at v1:
///           * Native Solidity (`OnChainAssessor`) — evaluates each predicate
///             directly on-chain. Cheap per-fill (~1-2k gas) but pays for the
///             slim-payload calldata.
///           * R0 STARK (`R0BoundlessAssessorAdapter`) — verifies an off-chain
///             merkle commitment proof. Fixed ~280k Groth16 verify per call,
///             amortized across all fills in the batch.
///
///         Brokers choose between them by setting the first 4 bytes of
///         `assessorSeal` to the registered adapter's selector. The router
///         dispatches accordingly; the market is unchanged.
///
/// @dev    Trust contract:
///         - Caller (market) has already verified each `SlimRequest`
///           reconstructs to the lock's stored `requestDigest`. Adapter MUST
///           trust the supplied `requests` as the signed request payload.
///         - `prover` is a universal arg: the market needs a trusted prover
///           for crediting / slashing; the adapter binds it via its own
///           mechanism (R0 STARK journal commitment; future signature payload;
///           etc.). Native on-chain adapter trusts `msg.sender`-equivalent at
///           the market layer.
///         - Terminal seam. Classes with this `interfaceTag` are referenced
///           by other classes' `requiredAssessorClass` and MUST never be
///           selected as a verifier class.
interface IBoundlessAssessor {
    /// @notice Verify per-fill predicate satisfaction.
    /// @param requests        Per-fill slim payloads (pre-verified by caller).
    /// @param fills           Per-fill `Fulfillment`s, same order.
    /// @param requestDigests  Pre-computed `requestDigest` per fill, same order.
    ///                        The market already reconstructed and binding-
    ///                        checked these against the lock / `FulfillmentContext`,
    ///                        so the adapter can use them directly. If a caller
    ///                        bypassing the market passes bad values, the
    ///                        adapter's binding mechanism (STARK journal /
    ///                        prover signature) will detect the mismatch.
    /// @param prover          Address the market credits / slashes.
    /// @param assessorSeal    Adapter-specific envelope (empty for on-chain).
    function verifyAssessor(
        SlimRequest[] calldata requests,
        Fulfillment[] calldata fills,
        bytes32[] calldata requestDigests,
        address prover,
        bytes calldata assessorSeal
    ) external view;
}
