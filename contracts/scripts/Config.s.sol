// Copyright (c) 2025 RISC Zero, Inc.
//
// All rights reserved.

pragma solidity ^0.8.20;

import {Vm} from "forge-std/Vm.sol";
import "forge-std/Test.sol";

struct DeploymentConfig {
    string name;
    uint256 chainId;
    address admin;
    address verifier;
    address setVerifier;
    address boundlessMarket;
    address hitPoints;
    bytes32 assessorImageId;
    string assessorGuestUrl;
}

library ConfigLoader {
    /// Reference the vm address without needing to inherit from Script.
    Vm private constant VM = Vm(0x7109709ECfa91a80626fF3989D68f67F5b1DD12D);

    function loadConfig(string memory configFilePath)
        internal
        view
        returns (string memory config, string memory chainKey)
    {
        // Load the config file
        config = VM.readFile(configFilePath);

        // Get the config profile from the environment variable, or leave it empty
        chainKey = VM.envOr("CHAIN_KEY", string(""));

        // If no profile is set, select the default one based on the chainId
        if (bytes(chainKey).length == 0) {
            string[] memory chainKeys = VM.parseTomlKeys(config, ".chains");
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
        deploymentConfig.verifier = stdToml.readAddress(config, string.concat(chain, ".verifier"));
        deploymentConfig.setVerifier = stdToml.readAddress(config, string.concat(chain, ".set-verifier"));
        deploymentConfig.boundlessMarket = stdToml.readAddress(config, string.concat(chain, ".boundless-market"));
        deploymentConfig.hitPoints = stdToml.readAddress(config, string.concat(chain, ".hit-points"));
        deploymentConfig.assessorImageId = stdToml.readBytes32(config, string.concat(chain, ".assessor-image-id"));
        deploymentConfig.assessorGuestUrl = stdToml.readString(config, string.concat(chain, ".assessor-guest-url"));

        return deploymentConfig;
    }
}
