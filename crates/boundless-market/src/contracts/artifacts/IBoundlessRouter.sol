// Copyright 2026 Boundless Foundation, Inc.
//
// Use of this source code is governed by the Business Source License
// as found in the LICENSE-BSL file.
// SPDX-License-Identifier: BUSL-1.1

pragma solidity ^0.8.26;

import {FulfillmentBatch} from "../../types/FulfillmentBatch.sol";

/// @title IBoundlessRouter — caller-facing seam for the verification engine.
///
/// @notice Surface the market (and any future fulfillment-flow caller) needs
///         to drive the router's per-batch dispatch. Governance + registration
///         entry points (`addClass`, `instantiate`, `removeClass`,
///         `removeEntry`) live on the concrete `BoundlessRouter` and are
///         called by admin tooling only, never by fulfill-path consumers.
///         Keeping the admin surface separate lets the market depend on the
///         abstract seam and lets external tooling (Rust bindings, etc.)
///         generate a smaller, fulfill-only binding.
interface IBoundlessRouter {
    /// @notice Verify all fills in one single-class fulfillment batch.
    /// @dev    Mirror of `BoundlessRouter.verifyBatch`. The two are kept
    ///         shape-identical so the assessor staticcall inside the router
    ///         can forward this function's calldata tail verbatim.
    function verifyBatch(FulfillmentBatch calldata batch, bytes32[] calldata requestDigests) external view;
}
