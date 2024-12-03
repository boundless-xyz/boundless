// Copyright 2024 RISC Zero, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

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
