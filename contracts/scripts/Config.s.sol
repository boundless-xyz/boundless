// Copyright 2025 RISC Zero, Inc.
//
// Use of this source code is governed by the Business Source License
// as found in the LICENSE-BSL file.

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
    address stakeToken;
    bytes32 assessorImageId;
    string assessorGuestUrl;
}

library ConfigLoader {
    /// Reference the vm address without needing to inherit from Script.
    Vm private constant VM = Vm(0x7109709ECfa91a80626fF3989D68f67F5b1DD12D);

    function loadConfig(string memory configFilePath)
        internal
        view
        returns (string memory config, string memory deployKey)
    {
        // Load the config file
        config = VM.readFile(configFilePath);

        // Get the config profile from the environment variable, or leave it empty
        string memory chainKey = VM.envOr("CHAIN_KEY", string(""));
        string memory stackTag = VM.envOr("STACK_TAG", string(""));
        if (bytes(stackTag).length == 0) {
            deployKey = chainKey;
        } else if (bytes(chainKey).length != 0) {
            deployKey = string.concat(chainKey, "-", stackTag);
        }

        // If no profile is set, select the default one based on the chainId
        if (bytes(deployKey).length == 0) {
            string[] memory deployKeys = VM.parseTomlKeys(config, ".deployment");
            for (uint256 i = 0; i < deployKeys.length; i++) {
                if (stdToml.readUint(config, string.concat(".deployment.", deployKeys[i], ".id")) == block.chainid) {
                    if (bytes(deployKey).length != 0) {
                        console2.log("Multiple entries found with chain ID %s", block.chainid);
                        require(false, "multiple entries found with same chain ID");
                    }
                    deployKey = deployKeys[i];
                }
            }
        }

        return (config, deployKey);
    }

    function loadDeploymentConfig(string memory configFilePath) internal view returns (DeploymentConfig memory) {
        (string memory config, string memory deployKey) = loadConfig(configFilePath);
        return ConfigParser.parseConfig(config, deployKey);
    }
}

library ConfigParser {
    function parseConfig(string memory config, string memory deployKey)
        internal
        view
        returns (DeploymentConfig memory)
    {
        DeploymentConfig memory deploymentConfig;

        string memory chain = string.concat(".deployment.", deployKey);

        deploymentConfig.name = stdToml.readString(config, string.concat(chain, ".name"));
        deploymentConfig.chainId = stdToml.readUint(config, string.concat(chain, ".id"));
        deploymentConfig.admin = stdToml.readAddressOr(config, string.concat(chain, ".admin"), address(0));
        deploymentConfig.verifier = stdToml.readAddressOr(config, string.concat(chain, ".verifier"), address(0));
        deploymentConfig.setVerifier = stdToml.readAddressOr(config, string.concat(chain, ".set-verifier"), address(0));
        deploymentConfig.boundlessMarket =
            stdToml.readAddressOr(config, string.concat(chain, ".boundless-market"), address(0));
        deploymentConfig.stakeToken = stdToml.readAddressOr(config, string.concat(chain, ".stake-token"), address(0));
        deploymentConfig.assessorImageId = stdToml.readBytes32(config, string.concat(chain, ".assessor-image-id"));
        deploymentConfig.assessorGuestUrl = stdToml.readString(config, string.concat(chain, ".assessor-guest-url"));

        return deploymentConfig;
    }
}
