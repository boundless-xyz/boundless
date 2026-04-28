// Copyright 2026 Boundless Foundation, Inc.
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

pragma solidity ^0.8.26;

import {Script, console2} from "forge-std/Script.sol";
import {SmartContractRequestor} from "../src/SmartContractRequestor.sol";

contract Deploy is Script {
    function run() external payable {
        uint256 key = vm.envUint("PRIVATE_KEY");
        address owner = vm.addr(key);
        address boundlessMarketAddress = vm.envAddress("BOUNDLESS_MARKET_ADDRESS");
        // Valid for any day since epoch — the example uses today's day-since-epoch as the request id.
        uint32 startDay = 0;
        uint32 endDay = type(uint32).max;

        vm.startBroadcast(key);
        SmartContractRequestor scr = new SmartContractRequestor(owner, boundlessMarketAddress, startDay, endDay);
        address scrAddress = address(scr);
        console2.log("Deployed SmartContractRequestor to", scrAddress);
        vm.stopBroadcast();
    }
}
