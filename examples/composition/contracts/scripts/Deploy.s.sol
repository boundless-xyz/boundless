// Copyright (c) 2025 RISC Zero, Inc.
//
// All rights reserved.

pragma solidity ^0.8.20;

import {Script, console2} from "forge-std/Script.sol";
import {IRiscZeroVerifier} from "risc0/IRiscZeroVerifier.sol";
import {Counter} from "../src/Counter.sol";

contract Deploy is Script {
    function run() external payable {
        // load ENV variables first
        uint256 key = vm.envUint("PRIVATE_KEY");
        address verifierAddress = vm.envAddress("VERIFIER_ADDRESS");
        vm.startBroadcast(key);

        IRiscZeroVerifier verifier = IRiscZeroVerifier(verifierAddress);
        Counter counter = new Counter(verifier);
        address counterAddress = address(counter);
        console2.log("Deployed Counter to", counterAddress);

        vm.stopBroadcast();
    }
}
