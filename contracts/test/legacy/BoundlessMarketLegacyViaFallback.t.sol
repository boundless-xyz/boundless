// Copyright 2026 Boundless Foundation, Inc.
//
// Use of this source code is governed by the Business Source License
// as found in the LICENSE-BSL file.

pragma solidity ^0.8.26;

import {UnsafeUpgrades} from "openzeppelin-foundry-upgrades/Upgrades.sol";

import {BoundlessMarket} from "../../src/legacy/BoundlessMarketLegacy.sol";
import {BoundlessMarket as BoundlessMarketNew} from "../../src/BoundlessMarket.sol";
import {IBoundlessRouter} from "../../src/router/interfaces/IBoundlessRouter.sol";
// ASSESSOR_IMAGE_ID and APP_JOURNAL are imported (and thereby re-exported) so that
// CrossABI.t.sol can keep sourcing them from this file, as it did when this file
// declared them itself.
import {
    BoundlessMarketLegacyTest,
    BoundlessMarketLegacyBasicTest,
    BoundlessMarketLegacyBench,
    BoundlessMarketLegacyUpgradeTest,
    ASSESSOR_IMAGE_ID,
    APP_JOURNAL,
    DEPRECATED_ASSESSOR_IMAGE_ID,
    DEPRECATED_ASSESSOR_DURATION
} from "./BoundlessMarketLegacy.t.sol";

/// @dev Re-runs the entire legacy ABI test battery from BoundlessMarketLegacy.t.sol,
///      but against the NEW market deployed in front of the legacy impl. Legacy-only
///      selectors reach the legacy bodies through BoundlessMarket.fallback(), while
///      selectors the new market declares execute on the new impl. Only the deployment
///      (_deployMarket) and the two signature-recovery expectations (which depend on the
///      proxy address) differ from the base suite; every test body is inherited.
///
/// @dev This base carries only the via-fallback deployment override; it declares no
///      tests of its own. The concrete suites below pick up the legacy test battery,
///      and CrossABI.t.sol extends this base to add cross-ABI-only invariants.
abstract contract BoundlessMarketLegacyViaFallbackTest is BoundlessMarketLegacyTest {
    function _deployMarket() internal virtual override {
        // Deploy the LEGACY implementation. This is what the fallback delegate-
        // calls into for any selector the new market does not declare.
        address legacyImpl = address(
            new BoundlessMarket(
                setVerifier,
                setVerifier,
                ASSESSOR_IMAGE_ID,
                DEPRECATED_ASSESSOR_IMAGE_ID,
                DEPRECATED_ASSESSOR_DURATION,
                address(collateralToken)
            )
        );

        // Deploy the NEW market impl and point the proxy at it. The new market's
        // router and fulfill-lib are set to non-zero placeholder addresses since
        // these tests exercise the legacy ABI surface, which is forwarded via
        // fallback before the new market's router / fulfill-lib are ever touched.
        boundlessMarketSource = address(
            new BoundlessMarketNew(
                IBoundlessRouter(address(0xdead)), address(collateralToken), legacyImpl, address(0xbeef)
            )
        );
        proxy = UnsafeUpgrades.deployUUPSProxy(
            boundlessMarketSource, abi.encodeCall(BoundlessMarketNew.initialize, (ownerWallet.addr))
        );
        // boundlessMarket is typed as the LEGACY contract so all calls below
        // emit the legacy ABI selectors. Selectors that exist on the new
        // market (lockRequest, slash, accounts, etc.) execute on the new
        // impl; legacy-only selectors (fulfill with the old shape,
        // imageInfo, verifyDelivery, etc.) fall through to the legacy impl
        // via fallback().
        boundlessMarket = BoundlessMarket(payable(proxy));
    }
}

contract BoundlessMarketLegacyViaFallbackBasicTest is
    BoundlessMarketLegacyBasicTest,
    BoundlessMarketLegacyViaFallbackTest
{
    // Disambiguate the diamond: _deployMarket is inherited both from the legacy base
    // (BoundlessMarketLegacyBasicTest) and the via-fallback override. Resolve to the
    // via-fallback deployment.
    function _deployMarket() internal override(BoundlessMarketLegacyTest, BoundlessMarketLegacyViaFallbackTest) {
        BoundlessMarketLegacyViaFallbackTest._deployMarket();
    }

    // The recovered addresses differ from the standalone legacy suite by one CREATE
    // nonce: _deployMarket here also deploys the new market impl before the proxy, so
    // the proxy address (and thus the EIP-712 domain separator) shifts. These are the
    // deterministic recoveries against this configuration.
    function _expectedIncorrectRequestSigner() internal pure override returns (address) {
        return address(0xf9D65aDD060EeC50A7e86C29d91fBEAaC0eDe727);
    }

    function _expectedIncorrectDomainSigner() internal pure override returns (address) {
        return address(0x27940eD27511Eef63A19320520D3fC30a4F35a56);
    }
}

contract BoundlessMarketLegacyViaFallbackBench is BoundlessMarketLegacyBench, BoundlessMarketLegacyViaFallbackTest {
    function _deployMarket() internal override(BoundlessMarketLegacyTest, BoundlessMarketLegacyViaFallbackTest) {
        BoundlessMarketLegacyViaFallbackTest._deployMarket();
    }
}

contract BoundlessMarketLegacyViaFallbackUpgradeTest is
    BoundlessMarketLegacyUpgradeTest,
    BoundlessMarketLegacyViaFallbackTest
{
    function _deployMarket() internal override(BoundlessMarketLegacyTest, BoundlessMarketLegacyViaFallbackTest) {
        BoundlessMarketLegacyViaFallbackTest._deployMarket();
    }
}
