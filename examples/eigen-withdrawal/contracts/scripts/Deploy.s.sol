// Copyright (c) 2024 RISC Zero, Inc.
//
// All rights reserved.

pragma solidity ^0.8.20;

import {Script, console2} from "forge-std/Script.sol";
import {IRiscZeroVerifier} from "risc0/IRiscZeroVerifier.sol";
import {EigenPod} from "../src/EigenPod.sol";

contract Deploy is Script {
    function run() external payable {
        // load ENV variables first
        uint256 key = vm.envUint("ADMIN_PRIVATE_KEY");
        address verifierAddress = vm.envAddress("SET_VERIFIER_ADDRESS");
        vm.startBroadcast(key);

        IRiscZeroVerifier verifier = IRiscZeroVerifier(verifierAddress);
        bytes32 salt = "12345";
        EigenPod pod = new EigenPod{salt: salt}(verifier);
        address podAddress = address(pod);
        console2.log("Deployed EigenPod to", podAddress);

        vm.stopBroadcast();
    }
}
