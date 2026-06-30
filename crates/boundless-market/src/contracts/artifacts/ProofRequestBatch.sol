// Copyright 2026 Boundless Foundation, Inc.
//
// Use of this source code is governed by the Business Source License
// as found in the LICENSE-BSL file.
// SPDX-License-Identifier: BUSL-1.1

pragma solidity ^0.8.26;

import {ProofRequest} from "./ProofRequest.sol";

/// @title ProofRequestBatch — group of unpriced/unlocked requests to price in one tx.
///
/// @notice Wraps the `ProofRequest[]` and matching client signatures that the
/// priced fulfillment paths (`priceAndFulfill`, `priceAndFulfillAndWithdraw`,
/// `submitRootAndPriceAndFulfill*`) consume. Mirrors `FulfillmentBatch` in shape
/// so the same-tx price-then-fulfill API reads symmetrically:
///
/// ```text
/// priceAndFulfill(ProofRequestBatch[] requestBatches,
///                 FulfillmentBatch[]  fulfillmentBatches)
/// ```
struct ProofRequestBatch {
    /// @notice Full `ProofRequest`s for the requests that need pricing this tx.
    ProofRequest[] requests;
    /// @notice Client signatures matching `requests` 1:1.
    bytes[] signatures;
}
