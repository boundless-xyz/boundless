// Copyright (c) 2024 RISC Zero, Inc.
//
// All rights reserved.

pragma solidity ^0.8.20;

import {Script, console2} from "forge-std/Script.sol";
import {IRiscZeroVerifier} from "risc0/IRiscZeroVerifier.sol";
import {ControlID, RiscZeroGroth16Verifier} from "risc0/groth16/RiscZeroGroth16Verifier.sol";
import {RiscZeroCheats} from "risc0/test/RiscZeroCheats.sol";

import {ProofMarket} from "../src/ProofMarket.sol";
import {RiscZeroSetVerifier} from "../src/RiscZeroSetVerifier.sol";

// For local testing:
import {ImageID as AssesorImgId} from "../src/AssessorImageID.sol";
import {ImageID as AggImgId} from "../src/AggregationSetImageID.sol";

contract Deploy is Script, RiscZeroCheats {
    function run() external {
        // load ENV variables first
        uint256 adminKey = vm.envUint("WALLET_PRIVATE_KEY");

        vm.startBroadcast(adminKey);

        IRiscZeroVerifier verifier = deployRiscZeroVerifier();

        string memory setBuilderGuestUrl = "";
        string memory assessorGuestUrl = "";
        if (bytes(vm.envOr("RISC0_DEV_MODE", string(""))).length > 0) {
            // TODO: Create a more robust way of getting a URI for guests.
            string memory cwd = vm.envString("PWD");
            setBuilderGuestUrl = string.concat(
                "file://",
                cwd,
                "/target/riscv-guest/riscv32im-risc0-zkvm-elf/release/aggregation-set-guest"
            );
            console2.log("Set builder URI", setBuilderGuestUrl);
            assessorGuestUrl = string.concat(
                "file://",
                cwd,
                "/target/riscv-guest/riscv32im-risc0-zkvm-elf/release/assessor-guest"
            );
            console2.log("Assessorr URI", assessorGuestUrl);
        }

        RiscZeroSetVerifier setVerifier = new RiscZeroSetVerifier(verifier, AggImgId.AGGREGATION_SET_GUEST_ID, setBuilderGuestUrl);
        console2.log("Deployed SetVerifier to,", address(setVerifier));

        ProofMarket market = new ProofMarket(setVerifier, AssesorImgId.ASSESSOR_GUEST_ID);
        console2.log("Deployed ProofMarket to", address(market));

        vm.stopBroadcast();
    }
}
