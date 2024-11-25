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
import {ConfigLoader, DeploymentConfig} from "./Config.s.sol";

import {BoundlessMarket} from "../src/BoundlessMarket.sol";
import {RiscZeroSetVerifier} from "../src/RiscZeroSetVerifier.sol";

contract Deploy is Script, RiscZeroCheats {
    // Path to deployment config file, relative to the project root.
    string constant CONFIG_FILE = "contracts/deployment.toml";

    IRiscZeroVerifier verifier;
    RiscZeroSetVerifier setVerifier;
    address boundlessMarketAddress;
    bytes32 setBuilderImageId;
    bytes32 assessorImageId;

    function run() external {
        string memory setBuilderGuestUrl = "";
        string memory assessorGuestUrl = "";

        // load ENV variables first
        uint256 deployerKey = vm.envOr("DEPLOYER_PRIVATE_KEY", uint256(0));
        require(deployerKey != 0, "No deployer key provided. Please set the env var DEPLOYER_PRIVATE_KEY.");
        vm.rememberKey(deployerKey);

        address boundlessMarketOwner = vm.envAddress("PROOF_MARKET_OWNER");
        console2.log("BoundlessMarket Owner:", boundlessMarketOwner);

        // Read and log the chainID
        uint256 chainId = block.chainid;
        console2.log("You are deploying on ChainID %d", chainId);

        // Load the deployment config
        DeploymentConfig memory deploymentConfig =
            ConfigLoader.loadDeploymentConfig(string.concat(vm.projectRoot(), "/", CONFIG_FILE));

        // Assign parsed config values to the variables
        verifier = IRiscZeroVerifier(deploymentConfig.router);
        setVerifier = RiscZeroSetVerifier(deploymentConfig.setVerifier);
        setBuilderImageId = deploymentConfig.setBuilderImageId;
        setBuilderGuestUrl = deploymentConfig.setBuilderGuestUrl;
        assessorImageId = deploymentConfig.assessorImageId;
        assessorGuestUrl = deploymentConfig.assessorGuestUrl;

        vm.startBroadcast(deployerKey);

        // Deploy the verifier, if not already deployed.
        if (address(verifier) == address(0)) {
            verifier = deployRiscZeroVerifier();
        } else {
            console2.log("Using IRiscZeroVerifier contract deployed at", address(verifier));
        }

        // Set the setBuilderImageId and assessorImageId if not set.
        if (setBuilderImageId == bytes32(0)) {
            revert("set builder image ID must be set in deployment.toml");
        }
        if (assessorImageId == bytes32(0)) {
            revert("assessor image ID must be set in deployment.toml");
        }

        if (bytes(vm.envOr("RISC0_DEV_MODE", string(""))).length > 0) {
            // TODO: Create a more robust way of getting a URI for guests, and ensure that it is
            // in-sync with the configured image ID.
            string memory setBuilderPath = "/target/riscv-guest/riscv32im-risc0-zkvm-elf/release/set-builder-guest";
            string memory cwd = vm.envString("PWD");
            setBuilderGuestUrl = string.concat("file://", cwd, setBuilderPath);
            console2.log("Set builder URI", setBuilderGuestUrl);

            string[] memory argv = new string[](4);
            argv[0] = "r0vm";
            argv[1] = "--id";
            argv[2] = "--elf";
            argv[3] = string.concat(".", setBuilderPath);
            setBuilderImageId = abi.decode(vm.ffi(argv), (bytes32));

            string memory assessorPath = "/target/riscv-guest/riscv32im-risc0-zkvm-elf/release/assessor-guest";
            assessorGuestUrl = string.concat("file://", cwd, assessorPath);
            console2.log("Assessor URI", assessorGuestUrl);

            argv[3] = string.concat(".", assessorPath);
            assessorImageId = abi.decode(vm.ffi(argv), (bytes32));
        }

        // Deploy the setVerifier, if not already deployed.
        if (address(setVerifier) == address(0)) {
            setVerifier = new RiscZeroSetVerifier(verifier, setBuilderImageId, setBuilderGuestUrl);
            console2.log("Deployed RiscZeroSetVerifier to", address(setVerifier));
        } else {
            console2.log("Using RiscZeroSetVerifier contract deployed at", address(setVerifier));
        }

        // Deploy the proof market
        address newImplementation = address(new BoundlessMarket(setVerifier, assessorImageId));
        console2.log("Deployed new BoundlessMarket implementation at", newImplementation);
        boundlessMarketAddress = UnsafeUpgrades.deployUUPSProxy(
            newImplementation, abi.encodeCall(BoundlessMarket.initialize, (boundlessMarketOwner, assessorGuestUrl))
        );
        console2.log("Deployed BoundlessMarket (proxy) to", boundlessMarketAddress);

        vm.stopBroadcast();
    }
}
