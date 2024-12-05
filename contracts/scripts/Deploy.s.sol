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

import {Script, console2} from "forge-std/Script.sol";
import "forge-std/Test.sol";
import {IRiscZeroVerifier} from "risc0/IRiscZeroVerifier.sol";
import {ControlID, RiscZeroGroth16Verifier} from "risc0/groth16/RiscZeroGroth16Verifier.sol";
import {RiscZeroSetVerifier} from "risc0/RiscZeroSetVerifier.sol";
import {RiscZeroCheats} from "risc0/test/RiscZeroCheats.sol";
import {ERC1967Proxy} from "@openzeppelin/contracts/proxy/ERC1967/ERC1967Proxy.sol";
import {ConfigLoader, DeploymentConfig} from "./Config.s.sol";
import {BoundlessMarket} from "../src/BoundlessMarket.sol";

contract Deploy is Script, RiscZeroCheats {
    // Path to deployment config file, relative to the project root.
    string constant CONFIG_FILE = "contracts/deployment.toml";

    IRiscZeroVerifier verifier;
    address boundlessMarketAddress;
    bytes32 assessorImageId;

    function run() external {
        string memory assessorGuestUrl = "";

        // load ENV variables first
        uint256 deployerKey = vm.envOr("DEPLOYER_PRIVATE_KEY", uint256(0));
        require(deployerKey != 0, "No deployer key provided. Please set the env var DEPLOYER_PRIVATE_KEY.");
        vm.rememberKey(deployerKey);

        address boundlessMarketOwner = vm.envAddress("BOUNDLESS_MARKET_OWNER");
        console2.log("BoundlessMarket Owner:", boundlessMarketOwner);

        // Read and log the chainID
        uint256 chainId = block.chainid;
        console2.log("You are deploying on ChainID %d", chainId);

        // Load the deployment config
        DeploymentConfig memory deploymentConfig =
            ConfigLoader.loadDeploymentConfig(string.concat(vm.projectRoot(), "/", CONFIG_FILE));

        // Assign parsed config values to the variables
        verifier = IRiscZeroVerifier(deploymentConfig.verifier);
        assessorImageId = deploymentConfig.assessorImageId;
        assessorGuestUrl = deploymentConfig.assessorGuestUrl;

        if (assessorImageId == bytes32(0)) {
            revert("assessor image ID must be set in deployment.toml");
        }

        vm.startBroadcast(deployerKey);

        // Deploy the verifier, if dev mode is enabled.
        if (bytes(vm.envOr("RISC0_DEV_MODE", string(""))).length > 0) {
            IRiscZeroVerifier _verifier = deployRiscZeroVerifier();

            // TODO: Create a more robust way of getting a URI for guests, and ensure that it is
            // in-sync with the configured image ID.
            string memory setBuilderPath =
                "/target/riscv-guest/guest-set-builder/set-builder/riscv32im-risc0-zkvm-elf/release/set-builder";
            string memory cwd = vm.envString("PWD");
            string memory setBuilderGuestUrl = string.concat("file://", cwd, setBuilderPath);
            console2.log("Set builder URI", setBuilderGuestUrl);

            string[] memory argv = new string[](4);
            argv[0] = "r0vm";
            argv[1] = "--id";
            argv[2] = "--elf";
            argv[3] = string.concat(".", setBuilderPath);
            bytes32 setBuilderImageId = abi.decode(vm.ffi(argv), (bytes32));

            string memory assessorPath =
                "/target/riscv-guest/guest-assessor/assessor-guest/riscv32im-risc0-zkvm-elf/release/assessor-guest";
            assessorGuestUrl = string.concat("file://", cwd, assessorPath);
            console2.log("Assessor URI", assessorGuestUrl);

            argv[3] = string.concat(".", assessorPath);
            assessorImageId = abi.decode(vm.ffi(argv), (bytes32));

            RiscZeroSetVerifier setVerifier = new RiscZeroSetVerifier(_verifier, setBuilderImageId, setBuilderGuestUrl);
            console2.log("Deployed RiscZeroSetVerifier to", address(setVerifier));
            verifier = IRiscZeroVerifier(setVerifier);
        }

        if (address(verifier) == address(0)) {
            revert("verifier must be specified in deployment.toml");
        } else {
            console2.log("Using IRiscZeroVerifier deployed at", address(verifier));
        }

        // Deploy the Boundless market
        bytes32 salt = bytes32(0);
        address newImplementation = address(new BoundlessMarket{salt: salt}(verifier, assessorImageId));
        console2.log("Deployed new BoundlessMarket implementation at", newImplementation);
        boundlessMarketAddress = address(
            new ERC1967Proxy{salt: salt}(
                newImplementation, abi.encodeCall(BoundlessMarket.initialize, (boundlessMarketOwner, assessorGuestUrl))
            )
        );
        console2.log("Deployed BoundlessMarket (proxy) to", boundlessMarketAddress);

        vm.stopBroadcast();
    }
}
