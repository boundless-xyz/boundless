// Copyright (c) 2024 RISC Zero, Inc.
//
// All rights reserved.

pragma solidity ^0.8.20;

import {Script} from "forge-std/Script.sol";
import "forge-std/Test.sol";

struct DeploymentConfig {
    string name;
    uint256 chainId;
    address admin;
    address router;
    address setVerifier;
    address proofMarket;
    bytes32 setBuilderImageId;
    string setBuilderGuestUrl;
    bytes32 assessorImageId;
    string assessorGuestUrl;
}

contract ConfigLoader is Script {
    function loadConfig(string memory configFilePath)
        internal
        view
        returns (string memory config, string memory chainKey)
    {
        // Load the config file
        config = vm.readFile(configFilePath);

        // Get the config profile from the environment variable, or leave it empty
        chainKey = vm.envOr("CHAIN_KEY", string(""));

        // If no profile is set, select the default one based on the chainId
        if (bytes(chainKey).length == 0) {
            string[] memory chainKeys = vm.parseTomlKeys(config, ".chains");
            for (uint256 i = 0; i < chainKeys.length; i++) {
                if (stdToml.readUint(config, string.concat(".chains.", chainKeys[i], ".id")) == block.chainid) {
                    chainKey = chainKeys[i];
                    break;
                }
            }
        }

        return (config, chainKey);
    }

    function loadDeploymentConfig(string memory configFilePath) internal view returns (DeploymentConfig memory) {
        (string memory config, string memory chainKey) = loadConfig(configFilePath);
        return ConfigParser.parseConfig(config, chainKey);
    }
}

library ConfigParser {
    function parseConfig(string memory config, string memory chainKey)
        internal
        pure
        returns (DeploymentConfig memory)
    {
        DeploymentConfig memory deploymentConfig;

        string memory chain = string.concat(".chains.", chainKey);

        deploymentConfig.name = stdToml.readString(config, string.concat(chain, ".name"));
        deploymentConfig.chainId = stdToml.readUint(config, string.concat(chain, ".id"));
        deploymentConfig.admin = stdToml.readAddress(config, string.concat(chain, ".admin"));
        deploymentConfig.router = stdToml.readAddress(config, string.concat(chain, ".router"));
        deploymentConfig.setVerifier = stdToml.readAddress(config, string.concat(chain, ".set-verifier"));
        deploymentConfig.proofMarket = stdToml.readAddress(config, string.concat(chain, ".proof-market"));
        deploymentConfig.setBuilderImageId = stdToml.readBytes32(config, string.concat(chain, ".set-builder-image-id"));
        deploymentConfig.setBuilderGuestUrl = stdToml.readString(config, string.concat(chain, ".set-builder-guest-url"));
        deploymentConfig.assessorImageId = stdToml.readBytes32(config, string.concat(chain, ".assessor-image-id"));
        deploymentConfig.assessorGuestUrl = stdToml.readString(config, string.concat(chain, ".assessor-guest-url"));

        return deploymentConfig;
    }
}
