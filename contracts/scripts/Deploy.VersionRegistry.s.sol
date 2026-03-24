// Copyright 2026 Boundless Foundation, Inc.
//
// Use of this source code is governed by the Business Source License
// as found in the LICENSE-BSL file.
// SPDX-License-Identifier: BUSL-1.1

pragma solidity ^0.8.26;

import {console2} from "forge-std/Script.sol";
import {VersionRegistry} from "../src/VersionRegistry.sol";
import {ConfigLoader, DeploymentConfig} from "./Config.s.sol";
import {ERC1967Proxy} from "@openzeppelin/contracts/proxy/ERC1967/ERC1967Proxy.sol";
import {BoundlessScriptBase, BoundlessScript} from "./BoundlessScript.s.sol";

contract DeployVersionRegistry is BoundlessScriptBase {
    struct DeployedContracts {
        address versionRegistryImpl;
        address versionRegistryAddress;
    }

    /// @notice Updates deployment.toml with deployed contract addresses.
    function updateDeploymentToml(DeployedContracts memory contracts) internal {
        console2.log("Updating deployment.toml with VersionRegistry contract addresses");

        string[] memory args = new string[](6);
        args[0] = "python3";
        args[1] = "contracts/update_deployment_toml.py";
        args[2] = "--version-registry";
        args[3] = vm.toString(contracts.versionRegistryAddress);
        args[4] = "--version-registry-impl";
        args[5] = vm.toString(contracts.versionRegistryImpl);
        vm.ffi(args);
    }

    function run() external {
        // Load ENV variables first
        uint256 deployerKey = vm.envOr("DEPLOYER_PRIVATE_KEY", uint256(0));
        require(
            deployerKey != 0,
            "No deployer key provided. Please set the env var DEPLOYER_PRIVATE_KEY. Ensure private key prefixed with 0x"
        );
        vm.rememberKey(deployerKey);

        console2.log("Deploying VersionRegistry contract (admin will be loaded from deployment.toml)");

        // Read and log the chainID
        uint256 chainId = block.chainid;
        console2.log("You are deploying on ChainID %d", chainId);

        // Load the deployment config
        DeploymentConfig memory deploymentConfig =
            ConfigLoader.loadDeploymentConfig(string.concat(vm.projectRoot(), "/", CONFIG));

        // Validate admin address is set
        address versionRegistryAdmin =
            BoundlessScript.requireLib(deploymentConfig.versionRegistryAdmin, "VersionRegistry admin");

        bytes32 salt = bytes32(uint256(keccak256("VersionRegistry")));

        vm.startBroadcast();

        // Deploy implementation
        address versionRegistryImpl = address(new VersionRegistry{salt: salt}());
        console2.log("Deployed VersionRegistry impl to", versionRegistryImpl);

        // Deploy proxy
        address versionRegistryAddress = address(
            new ERC1967Proxy{salt: salt}(
                versionRegistryImpl, abi.encodeCall(VersionRegistry.initialize, (versionRegistryAdmin))
            )
        );
        console2.log("Deployed VersionRegistry proxy to", versionRegistryAddress);
        console2.log("VersionRegistry admin:", versionRegistryAdmin);

        vm.stopBroadcast();

        // Update deployment.toml with contract addresses
        DeployedContracts memory deployedContracts = DeployedContracts({
            versionRegistryImpl: versionRegistryImpl, versionRegistryAddress: versionRegistryAddress
        });
        updateDeploymentToml(deployedContracts);

        console2.log("VersionRegistry deployed successfully!");
        console2.log("VersionRegistry:", versionRegistryAddress);

        // Check for uncommitted changes warning
        checkUncommittedChangesWarning("Deployment");
    }
}
