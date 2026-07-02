// Copyright 2026 Boundless Foundation, Inc.
//
// Use of this source code is governed by the Business Source License
// as found in the LICENSE-BSL file.

pragma solidity ^0.8.26;

import {Test} from "forge-std/Test.sol";
import {Upgrades} from "openzeppelin-foundry-upgrades/Upgrades.sol";
import {Options as UpgradeOptions} from "openzeppelin-foundry-upgrades/Options.sol";

contract UpgradeTest is Test {
    function testUpgradeability() public {
        UpgradeOptions memory opts;
        // File-qualify the reference: the legacy fallback source (src/legacy/BoundlessMarketLegacy.sol)
        // also declares a contract named `BoundlessMarket`, so the bare name is ambiguous in any
        // reference build that includes contracts/src/legacy/. Match the qualified new-side lookup below.
        opts.referenceContract = "build-info-reference:contracts/src/BoundlessMarket.sol:BoundlessMarket";
        opts.referenceBuildInfoDir = "contracts/reference-contract/build-info-reference";
        Upgrades.validateUpgrade("BoundlessMarket.sol:BoundlessMarket", opts);
    }
}
