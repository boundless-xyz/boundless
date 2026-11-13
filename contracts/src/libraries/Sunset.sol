// Copyright 2026 Boundless Foundation, Inc.
//
// Use of this source code is governed by the Business Source License
// as found in the LICENSE-BSL file.

pragma solidity ^0.8.26;

import {IBoundlessMarket as ILegacyMarket} from "boundless-market-legacy/IBoundlessMarketLegacy.sol";

/// @dev No proxy storage is added by the one-off November 5 shutdown.
library Sunset {
    uint256 internal constant TIMESTAMP = 1793836800;
    error ProtocolSunset();

    function requireActive() internal view {
        if (block.timestamp >= TIMESTAMP) revert ProtocolSunset();
    }

    function requireActiveLegacy(bytes4 selector) internal view {
        if (block.timestamp < TIMESTAMP) return;
        // Preserve the known legacy reads and admin URL setter; unknown fallback calls fail closed.
        if (
            selector == ILegacyMarket.verifyDelivery.selector || selector == ILegacyMarket.imageInfo.selector
                || selector == bytes4(keccak256("VERIFIER()"))
                || selector == bytes4(keccak256("APPLICATION_VERIFIER()"))
                || selector == bytes4(keccak256("ASSESSOR_ID()"))
                || selector == bytes4(keccak256("DEPRECATED_ASSESSOR_ID()"))
                || selector == bytes4(keccak256("DEPRECATED_ASSESSOR_EXPIRES_AT()"))
                || selector == bytes4(keccak256("DEFAULT_MAX_GAS_FOR_VERIFY()"))
                || selector == bytes4(keccak256("setImageUrl(string)"))
        ) return;
        revert ProtocolSunset();
    }
}
