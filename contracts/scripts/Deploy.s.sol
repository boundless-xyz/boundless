// Copyright (c) 2024 RISC Zero, Inc.
//
// All rights reserved.

pragma solidity ^0.8.20;

import {Script, console2} from "forge-std/Script.sol";
import "forge-std/Test.sol";
import {IRiscZeroVerifier} from "risc0/IRiscZeroVerifier.sol";
import {ControlID, RiscZeroGroth16Verifier} from "risc0/groth16/RiscZeroGroth16Verifier.sol";
import {RiscZeroCheats} from "risc0/test/RiscZeroCheats.sol";
import {UnsafeUpgrades, Upgrades} from "openzeppelin-foundry-upgrades/Upgrades.sol";

import {ProofMarket} from "../src/ProofMarket.sol";
import {RiscZeroSetVerifier} from "../src/RiscZeroSetVerifier.sol";

// For local testing:
import {ImageID as AssesorImgId} from "../src/AssessorImageID.sol";
import {ImageID as SetBuidlerId} from "../src/SetBuilderImageID.sol";

contract Deploy is Script, RiscZeroCheats {
    // Path to deployment config file, relative to the project root.
    string constant CONFIG_FILE = "contracts/scripts/config.toml";

    IRiscZeroVerifier verifier;
    RiscZeroSetVerifier setVerifier;
    address proofMarketAddress;
    bytes32 setBuilderImageId;
    bytes32 assessorImageId;

    function run() external {
        string memory setBuilderGuestUrl = "";
        string memory assessorGuestUrl = "";

        // load ENV variables first
        uint256 adminKey = vm.envUint("PRIVATE_KEY");

        // Read and log the chainID
        uint256 chainId = block.chainid;
        console2.log("You are deploying on ChainID %d", chainId);

        // Read the config profile from the environment variable, or use the default for the chainId.
        // Default is the first profile with a matching chainId field.
        string memory config = vm.readFile(string.concat(vm.projectRoot(), "/", CONFIG_FILE));
        string memory configProfile = vm.envOr("CONFIG_PROFILE", string(""));
        if (bytes(configProfile).length == 0) {
            string[] memory profileKeys = vm.parseTomlKeys(config, ".profile");
            for (uint256 i = 0; i < profileKeys.length; i++) {
                if (stdToml.readUint(config, string.concat(".profile.", profileKeys[i], ".chainId")) == chainId) {
                    configProfile = profileKeys[i];
                    break;
                }
            }
        }
        
        if (bytes(configProfile).length != 0) {
            console2.log("Deploying using config profile:", configProfile);
            string memory configProfileKey = string.concat(".profile.", configProfile);
            address riscZeroVerifierAddress =
                stdToml.readAddress(config, string.concat(configProfileKey, ".riscZeroVerifierAddress"));
            // If set, use the predeployed verifier address found in the config.
            verifier = IRiscZeroVerifier(riscZeroVerifierAddress);
            
            address riscZeroSetVerifierAddress =
                stdToml.readAddress(config, string.concat(configProfileKey, ".riscZeroSetVerifierAddress"));
            // If set, use the predeployed setVerifier address found in the config.
            setVerifier = RiscZeroSetVerifier(riscZeroSetVerifierAddress);

            // If set, use the predeployed proof market (proxy) address found in the config.
            proofMarketAddress = stdToml.readAddress(config, string.concat(configProfileKey, ".proofMarketAddress"));
        
            setBuilderImageId = stdToml.readBytes32(config, string.concat(configProfileKey, ".setBuilderImageId"));
            setBuilderGuestUrl = stdToml.readString(config, string.concat(configProfileKey, ".setBuilderGuestUrl"));
            assessorImageId = stdToml.readBytes32(config, string.concat(configProfileKey, ".assessorImageId"));
            assessorGuestUrl = stdToml.readString(config, string.concat(configProfileKey, ".assessorGuestUrl"));
        }

        vm.startBroadcast(adminKey);

        // Deploy the verifier, if not already deployed.
        if (address(verifier) == address(0)) {
            verifier = deployRiscZeroVerifier();
        } else {
            console2.log("Using IRiscZeroVerifier contract deployed at", address(verifier));
        }

        // Set the setBuilderImageId and assessorImageId if not set.
        if (setBuilderImageId == bytes32(0)) {
            setBuilderImageId = SetBuidlerId.SET_BUILDER_GUEST_ID;
        }
        if (assessorImageId == bytes32(0)) {
            assessorImageId = AssesorImgId.ASSESSOR_GUEST_ID;
        }

        if (bytes(vm.envOr("RISC0_DEV_MODE", string(""))).length > 0) {
            // TODO: Create a more robust way of getting a URI for guests.
            string memory cwd = vm.envString("PWD");
            setBuilderGuestUrl = string.concat(
                "file://",
                cwd,
                "/target/riscv-guest/riscv32im-risc0-zkvm-elf/release/set-builder-guest"
            );
            console2.log("Set builder URI", setBuilderGuestUrl);
            assessorGuestUrl = string.concat(
                "file://",
                cwd,
                "/target/riscv-guest/riscv32im-risc0-zkvm-elf/release/assessor-guest"
            );
            console2.log("Assessor URI", assessorGuestUrl);
        }

        // Deploy the setVerifier, if not already deployed.
        if (address(setVerifier) == address(0)) {
            setVerifier = new RiscZeroSetVerifier(verifier, setBuilderImageId, setBuilderGuestUrl);
            console2.log("Deployed RiscZeroSetVerifier to", address(setVerifier));
        } else {
            console2.log("Using RiscZeroSetVerifier contract deployed at", address(setVerifier));
        }


        // Deploy the proof market
        address newImplementation = address(new ProofMarket());
        console2.log("Deployed new ProofMarket implementation at", newImplementation);
        
        // Deploy the proof market proxy if not already deployed.
        // Otherwise, upgrade the existing proxy.
        if (address(proofMarketAddress) == address(0)) {
            proofMarketAddress = UnsafeUpgrades.deployUUPSProxy(
                newImplementation,
                abi.encodeCall(
                    ProofMarket.initialize, (vm.addr(adminKey), setVerifier, assessorImageId, assessorGuestUrl)
                )
            );
            console2.log("Deployed ProofMarket (proxy) to", proofMarketAddress);
        } else {
            UnsafeUpgrades.upgradeProxy(proofMarketAddress, newImplementation, abi.encodeCall(
                    ProofMarket.upgrade, (assessorImageId, assessorGuestUrl)
                ), vm.addr(adminKey));
            console2.log("Upgraded ProofMarket (proxy) contract at", proofMarketAddress);
        }

        vm.stopBroadcast();
    }
}
