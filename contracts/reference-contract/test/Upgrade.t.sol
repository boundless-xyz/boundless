// Copyright (c) 2024 RISC Zero, Inc.
//
// All rights reserved.

pragma solidity ^0.8.20;

import {Test} from "forge-std/Test.sol";
import {Upgrades} from "openzeppelin-foundry-upgrades/Upgrades.sol";
import {Options as UpgradeOptions} from "openzeppelin-foundry-upgrades/Options.sol";

contract UpgradeTest is Test {
    function testUpgradeability() public {
        UpgradeOptions memory opts;
        opts.referenceContract = "build-info-reference:BoundlessMarket";
        opts.referenceBuildInfoDir = "contracts/reference-contract/build-info-reference";
        Upgrades.validateUpgrade("BoundlessMarket.sol:BoundlessMarket", opts);
    }
}
