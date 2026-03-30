// Copyright 2026 Boundless Foundation, Inc.
//
// Use of this source code is governed by the Business Source License
// as found in the LICENSE-BSL file.

pragma solidity ^0.8.26;

import {Vm} from "forge-std/Vm.sol";
import {console2, stdToml} from "forge-std/Test.sol";

struct DeploymentConfig {
    string name;
    uint256 chainId;
    address admin;
    address admin2;
    address verifier;
    address applicationVerifier;
    address setVerifier;
    address boundlessMarket;
    address boundlessMarketImpl;
    address boundlessMarketOldImpl;
    address collateralToken;
    bytes32 assessorImageId;
    string assessorGuestUrl;
    uint32 deprecatedAssessorDuration;
    // PoVW contract addresses
    address povwAccounting;
    address povwAccountingImpl;
    address povwAccountingOldImpl;
    address povwAccountingAdmin;
    string povwAccountingDeploymentCommit;
    address povwMint;
    address povwMintImpl;
    address povwMintOldImpl;
    address povwMintAdmin;
    string povwMintDeploymentCommit;
    // PoVW image IDs
    bytes32 povwLogUpdaterId;
    bytes32 povwMintCalculatorId;
    // ZKC contract addresses
    address zkc;
    address vezkc;
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

        console2.log("Using chain deployment key: %s", deployKey);

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
        deploymentConfig.admin2 = stdToml.readAddressOr(config, string.concat(chain, ".admin-2"), address(0));
        deploymentConfig.verifier = stdToml.readAddressOr(config, string.concat(chain, ".verifier"), address(0));
        deploymentConfig.applicationVerifier =
            stdToml.readAddressOr(config, string.concat(chain, ".application-verifier"), address(0));
        deploymentConfig.setVerifier = stdToml.readAddressOr(config, string.concat(chain, ".set-verifier"), address(0));
        deploymentConfig.boundlessMarket =
            stdToml.readAddressOr(config, string.concat(chain, ".boundless-market"), address(0));
        deploymentConfig.boundlessMarketImpl =
            stdToml.readAddressOr(config, string.concat(chain, ".boundless-market-impl"), address(0));
        deploymentConfig.boundlessMarketOldImpl =
            stdToml.readAddressOr(config, string.concat(chain, ".boundless-market-old-impl"), address(0));
        deploymentConfig.collateralToken =
            stdToml.readAddressOr(config, string.concat(chain, ".collateral-token"), address(0));
        deploymentConfig.assessorImageId = stdToml.readBytes32(config, string.concat(chain, ".assessor-image-id"));
        deploymentConfig.assessorGuestUrl = stdToml.readString(config, string.concat(chain, ".assessor-guest-url"));
        deploymentConfig.deprecatedAssessorDuration =
            uint32(stdToml.readUint(config, string.concat(chain, ".deprecated-assessor-duration")));

        // PoVW contract addresses
        deploymentConfig.povwAccounting =
            stdToml.readAddressOr(config, string.concat(chain, ".povw-accounting"), address(0));
        deploymentConfig.povwAccountingImpl =
            stdToml.readAddressOr(config, string.concat(chain, ".povw-accounting-impl"), address(0));
        deploymentConfig.povwAccountingOldImpl =
            stdToml.readAddressOr(config, string.concat(chain, ".povw-accounting-old-impl"), address(0));
        deploymentConfig.povwAccountingAdmin =
            stdToml.readAddressOr(config, string.concat(chain, ".povw-accounting-admin"), address(0));
        deploymentConfig.povwAccountingDeploymentCommit =
            stdToml.readStringOr(config, string.concat(chain, ".povw-accounting-deployment-commit"), "");
        deploymentConfig.povwMint = stdToml.readAddressOr(config, string.concat(chain, ".povw-mint"), address(0));
        deploymentConfig.povwMintImpl =
            stdToml.readAddressOr(config, string.concat(chain, ".povw-mint-impl"), address(0));
        deploymentConfig.povwMintOldImpl =
            stdToml.readAddressOr(config, string.concat(chain, ".povw-mint-old-impl"), address(0));
        deploymentConfig.povwMintAdmin =
            stdToml.readAddressOr(config, string.concat(chain, ".povw-mint-admin"), address(0));
        deploymentConfig.povwMintDeploymentCommit =
            stdToml.readStringOr(config, string.concat(chain, ".povw-mint-deployment-commit"), "");

        // PoVW image IDs
        deploymentConfig.povwLogUpdaterId =
            stdToml.readBytes32Or(config, string.concat(chain, ".povw-log-updater-id"), bytes32(0));
        deploymentConfig.povwMintCalculatorId =
            stdToml.readBytes32Or(config, string.concat(chain, ".povw-mint-calculator-id"), bytes32(0));

        // ZKC contract addresses
        deploymentConfig.zkc = stdToml.readAddressOr(config, string.concat(chain, ".zkc"), address(0));
        deploymentConfig.vezkc = stdToml.readAddressOr(config, string.concat(chain, ".vezkc"), address(0));

        return deploymentConfig;
    }
}
