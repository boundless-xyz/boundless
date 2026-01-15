// Copyright 2026 Boundless Foundation, Inc.
//
// Use of this source code is governed by the Business Source License
// as found in the LICENSE-BSL file.
// SPDX-License-Identifier: BUSL-1.1

pragma solidity ^0.8.20;

import {Vm} from "forge-std/Vm.sol";
import {console2} from "forge-std/console2.sol";
import {stdToml} from "forge-std/StdToml.sol";
import {SafeCast} from "@openzeppelin/contracts/utils/math/SafeCast.sol";

/// Deployment of a single verifier.
///
/// Many verifiers may be part of a deployment, with the router serving the purpose of making them
/// all accessible at a single address.
struct VerifierDeployment {
    string name;
    string version;
    bytes4 selector;
    address verifier;
    address estop;
    /// Specifies that this verifier is not deployed to the verifier router.
    /// Default is false since most of the verifiers in the config are intended to be routable.
    bool unroutable;
    /// Flag set when the verifier has had its estop activated. Once activated,
    /// the estop verifier is permanently inoperable.
    bool stopped;
}

/// Deployment of the verifier contracts on a particular chain.
///
/// The deployment.toml file contains a number of deployments. Each is indexed by a "chain key",
/// such as "ethereum-mainnet". This struct represents the values in one of those deployments.
struct Deployment {
    /// A friendly name for the network, such as "Ethereum Mainnet".
    string name;
    /// Chain ID for the network.
    uint256 chainId;
    /// Admin address for emergency stop contracts on this network, as well as the proposer for the
    /// timelock controller that acts as the admin for the router.
    address admin;
    /// Address of the verifier router in this deployment.
    address router;
    /// Address of the parent verifier router in this deployment.
    address parentRouter;
    /// Address of the timelock control in this deployment, which is set as the admin of the router.
    address timelockController;
    /// Min delay configured on the timelock controller.
    uint256 timelockDelay;
    /// Deployed verifier implementations.
    VerifierDeployment[] verifiers;
}

library DeploymentLib {
    /// Copy the deployment from memory to storage.
    /// Solidity does not allow this to be done via the assignment operator.
    function copyTo(Deployment memory mem, Deployment storage stor) internal {
        stor.name = mem.name;
        stor.chainId = mem.chainId;
        stor.admin = mem.admin;
        stor.router = mem.router;
        stor.parentRouter = mem.parentRouter;
        stor.timelockController = mem.timelockController;
        stor.timelockDelay = mem.timelockDelay;
        delete stor.verifiers;
        for (uint256 i = 0; i < mem.verifiers.length; i++) {
            stor.verifiers.push(mem.verifiers[i]);
        }
    }
}

library ConfigLoader {
    /// Reference the vm address without needing to inherit from Script.
    Vm private constant VM = Vm(0x7109709ECfa91a80626fF3989D68f67F5b1DD12D);

    /// Given the contents of the deployment.toml file, determine the active chain key.
    /// This function first checks the "CHAIN_KEY" environment variable and uses the value if set.
    /// If not set, this function looks for a deployment in the given TOML with a matching chainId
    /// field and returns the first matching result.
    function determineChainKey(string memory configToml) internal view returns (string memory) {
        // Get the config profile from the environment variable, or leave it empty
        string memory chainKey = VM.envOr("CHAIN_KEY", string(""));

        if (bytes(chainKey).length != 0) {
            console2.log("Using chain key %s set via environment variable", chainKey);
        } else {
            // Since no chain key is set, select the default one based on the chainId
            console2.log("Determining chain key from chain ID %d", block.chainid);
            string[] memory chainKeys = VM.parseTomlKeys(configToml, ".chains");
            for (uint256 i = 0; i < chainKeys.length; i++) {
                if (stdToml.readUint(configToml, string.concat(".chains.", chainKeys[i], ".id")) == block.chainid) {
                    chainKey = chainKeys[i];
                    console2.log("Using chain key %s from the config for chain ID %d", chainKey, block.chainid);
                    break;
                }
            }
        }
        require(bytes(chainKey).length != 0, "failed to determine the chain key in config TOML");

        // Double check that there chain-key and connected chain ID match. TODO: Is this too restrictive?
        uint256 chainId = stdToml.readUint(configToml, string.concat(".chains.", chainKey, ".id"));
        require(
            chainId == block.chainid, "chosen chain key is associated with chain ID that does not match connected chain"
        );

        return chainKey;
    }

    function loadDeploymentConfig(string memory configFilePath) internal view returns (Deployment memory) {
        string memory configToml = VM.readFile(configFilePath);
        string memory chainKey = determineChainKey(configToml);
        return ConfigParser.parseConfig(configToml, chainKey);
    }
}

library ConfigParser {
    using SafeCast for uint256;

    /// Reference the vm address without needing to inherit from Script.
    Vm private constant VM = Vm(0x7109709ECfa91a80626fF3989D68f67F5b1DD12D);

    function parseConfig(string memory config, string memory chainKey) internal view returns (Deployment memory) {
        string memory chain = string.concat(".chains.", chainKey);

        Deployment memory deploymentConfig;
        deploymentConfig.name = stdToml.readString(config, string.concat(chain, ".name"));
        deploymentConfig.chainId = stdToml.readUint(config, string.concat(chain, ".id"));
        deploymentConfig.admin = stdToml.readAddressOr(config, string.concat(chain, ".admin"), address(0));
        deploymentConfig.router = stdToml.readAddressOr(config, string.concat(chain, ".router"), address(0));
        deploymentConfig.parentRouter = stdToml.readAddress(config, string.concat(chain, ".parent-router"));
        deploymentConfig.timelockController =
            stdToml.readAddressOr(config, string.concat(chain, ".timelock-controller"), address(0));
        deploymentConfig.timelockDelay = stdToml.readUint(config, string.concat(chain, ".timelock-delay"));

        // Iterate over the verifier struct entries to get the length;
        // NOTE: We do this because Solidity doesn't support dynamic arrays in memory :|
        uint256 verifiersLength = 0;
        string memory verifierKey = string.concat(chain, ".verifiers[", VM.toString(verifiersLength), "]");
        while (stdToml.keyExists(config, verifierKey)) {
            verifiersLength++;
            verifierKey = string.concat(chain, ".verifiers[", VM.toString(verifiersLength), "]");
        }
        deploymentConfig.verifiers = new VerifierDeployment[](verifiersLength);

        // Iterate over the verifier struct entries and parse them.
        uint256 verifierIndex = 0;
        verifierKey = string.concat(chain, ".verifiers[", VM.toString(verifierIndex), "]");
        while (stdToml.keyExists(config, verifierKey)) {
            VerifierDeployment memory verifier;
            verifier.name = stdToml.readStringOr(config, string.concat(verifierKey, ".name"), "");
            verifier.version = stdToml.readStringOr(config, string.concat(verifierKey, ".version"), "");
            verifier.selector = bytes4(stdToml.readUint(config, string.concat(verifierKey, ".selector")).toUint32());
            verifier.verifier = stdToml.readAddress(config, string.concat(verifierKey, ".verifier"));
            verifier.estop = stdToml.readAddress(config, string.concat(verifierKey, ".estop"));
            verifier.unroutable = stdToml.readBoolOr(config, string.concat(verifierKey, ".unroutable"), false);
            verifier.stopped = stdToml.readBoolOr(config, string.concat(verifierKey, ".stopped"), false);

            deploymentConfig.verifiers[verifierIndex] = verifier;

            verifierIndex++;
            verifierKey = string.concat(chain, ".verifiers[", VM.toString(verifierIndex), "]");
        }

        return deploymentConfig;
    }
}
