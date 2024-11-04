// Copyright (c) 2024 RISC Zero, Inc.
//
// All rights reserved.

pragma solidity ^0.8.9;

import {Script} from "forge-std/Script.sol";
import {console2} from "forge-std/console2.sol";
import {Strings} from "openzeppelin/contracts/utils/Strings.sol";
import {RiscZeroVerifierRouter} from "risc0/RiscZeroVerifierRouter.sol";
import {RiscZeroVerifierEmergencyStop} from "risc0/RiscZeroVerifierEmergencyStop.sol";
import {IRiscZeroVerifier} from "risc0/IRiscZeroVerifier.sol";
import {RiscZeroSetVerifier} from "../src/RiscZeroSetVerifier.sol";
import {ProofMarket} from "../src/ProofMarket.sol";
import {ConfigLoader, DeploymentConfig, ConfigParser} from "./Config.s.sol";
import {ImageID as SetBuidlerId} from "../src/SetBuilderImageID.sol";
import {UnsafeUpgrades, Upgrades} from "openzeppelin-foundry-upgrades/Upgrades.sol";

/// @notice Base contract for the scripts below, providing common context and functions.
contract RiscZeroManagementScript is Script, ConfigLoader {
    // Path to deployment config file, relative to the project root.
    string constant CONFIG = "contracts/deployment.toml";

    /// @notice Returns the address of the deployer, set in the DEPLOYER_PUBLIC_KEY env var.
    function deployerAddress() internal returns (address) {
        address deployer = vm.envAddress("DEPLOYER_PUBLIC_KEY");
        uint256 deployerKey = vm.envOr("DEPLOYER_PRIVATE_KEY", uint256(0));
        if (deployerKey != 0) {
            require(vm.addr(deployerKey) == deployer, "DEPLOYER_PUBLIC_KEY and DEPLOYER_PRIVATE_KEY are inconsistent");
            vm.rememberKey(deployerKey);
        }
        return deployer;
    }
}

/// @notice Deployment script for the RISC Zero SetVerifier with Emergency Stop mechanism.
/// @dev Use the following environment variable to control the deployment:
///     * VERIFIER_ESTOP_OWNER owner of the emergency stop contract
///
/// See the Foundry documentation for more information about Solidity scripts.
/// https://book.getfoundry.sh/tutorials/solidity-scripting
contract DeployEstopSetVerifier is RiscZeroManagementScript {
    IRiscZeroVerifier _router;
    RiscZeroSetVerifier _setVerifier;
    RiscZeroVerifierEmergencyStop _verifierEstop;

    function run() external {
        address verifierEstopOwner = vm.envAddress("VERIFIER_ESTOP_OWNER");
        console2.log("verifierEstopOwner:", verifierEstopOwner);

        // Read and log the chainID
        uint256 chainId = block.chainid;
        console2.log("You are deploying on ChainID %d", chainId);

        // Load the config and chainKey
        string memory chainKey = vm.envOr("CHAIN_KEY", string(""));
        DeploymentConfig memory deploymentConfig =
            ConfigLoader.loadDeploymentConfig(string.concat(vm.projectRoot(), "/", CONFIG));
        _router = IRiscZeroVerifier(deploymentConfig.router);

        // Use a pre-deployed verifier, if not abort.
        require(address(_router) != address(0), "An IRiscZeroVerifier contract must be specified");
        console2.log("Using IRiscZeroVerifier contract deployed at", address(_router));

        vm.broadcast(deployerAddress());
        _setVerifier =
            new RiscZeroSetVerifier(_router, deploymentConfig.setBuilderImageId, deploymentConfig.setBuilderGuestUrl);
        console2.log("Deployed RiscZeroSetVerifier to", address(_setVerifier));

        vm.broadcast(deployerAddress());
        _verifierEstop = new RiscZeroVerifierEmergencyStop(IRiscZeroVerifier(address(_setVerifier)), verifierEstopOwner);

        // Print in TOML format
        console2.log("");
        console2.log(string.concat("[[chains.", chainKey, ".verifiers]]"));
        console2.log("name = RiscZeroSetVerifier");
        console2.log(string.concat("version = \"", _setVerifier.VERSION(), "\""));
        console2.log(
            string.concat("selector = \"", Strings.toHexString(uint256(uint32(_setVerifier.SELECTOR())), 4), "\"")
        );
        console2.log(string.concat("verifier = \"", Strings.toHexString(uint256(uint160(address(_setVerifier)))), "\""));
        console2.log(string.concat("estop = \"", Strings.toHexString(uint256(uint160(address(_verifierEstop)))), "\""));
    }
}

/// @notice Deployment script for the Proof Market deployment.
/// @dev Use the following environment variable to control the deployment:
///     * PROOF_MARKET_OWNER owner of the ProofMarket contract
///
/// See the Foundry documentation for more information about Solidity scripts.
/// https://book.getfoundry.sh/tutorials/solidity-scripting
contract DeployProofMarket is RiscZeroManagementScript {
    function run() external {
        address proofMarketOwner = vm.envAddress("PROOF_MARKET_OWNER");
        console2.log("proofMarketOwner:", proofMarketOwner);

        vm.broadcast(deployerAddress());
        // Deploy the proof market implementation
        address newImplementation = address(new ProofMarket());
        console2.log("Deployed new ProofMarket implementation at", newImplementation);

        // Load the config
        DeploymentConfig memory deploymentConfig =
            ConfigLoader.loadDeploymentConfig(string.concat(vm.projectRoot(), "/", CONFIG));

        address router = deploymentConfig.router;
        require(router != address(0), "RiscZeroVerifierRouter address must be set");
        console2.log("Using RiscZeroVerifierRouter at address", router);
        bytes32 assessorImageId = deploymentConfig.assessorImageId;
        require(assessorImageId != bytes32(0), "Assessor image ID must be set");
        string memory assessorGuestUrl = deploymentConfig.assessorGuestUrl;
        require(bytes(assessorGuestUrl).length != 0, "Assessor guest URL must be set");
        console2.log("Assessor info:");
        console2.logBytes32(assessorImageId);
        console2.logString(assessorGuestUrl);

        vm.broadcast(deployerAddress());
        // Deploy the proxy contract and initialize the contract
        // TODO: Switch to using the Upgrades library once the development is more stable.
        address proofMarketAddress = UnsafeUpgrades.deployUUPSProxy(
            newImplementation,
            abi.encodeCall(
                ProofMarket.initialize, (proofMarketOwner, IRiscZeroVerifier(router), assessorImageId, assessorGuestUrl)
            )
        );

        console2.log("Deployed ProofMarket (proxy) contract at", proofMarketAddress);
    }
}

/// @notice Deployment script for the Proof Market upgrade.
/// @dev Use the following environment variable to control the deployment:
///     * PROOF_MARKET_OWNER owner of the ProofMarket contract
///
/// See the Foundry documentation for more information about Solidity scripts.
/// https://book.getfoundry.sh/tutorials/solidity-scripting
contract UpgradeProofMarket is RiscZeroManagementScript {
    function run() external {
        address proofMarketOwner = vm.envAddress("PROOF_MARKET_OWNER");
        console2.log("proofMarketOwner:", proofMarketOwner);

        vm.broadcast(deployerAddress());
        // Deploy the proof market implementation
        address newImplementation = address(new ProofMarket());
        console2.log("Deployed new ProofMarket implementation at", newImplementation);

        // Load the config
        DeploymentConfig memory deploymentConfig =
            ConfigLoader.loadDeploymentConfig(string.concat(vm.projectRoot(), "/", CONFIG));
        address proofMarketAddress = deploymentConfig.proofMarket;
        require(proofMarketAddress != address(0), "ProofMarket (proxy) address must be set");
        console2.log("Using ProofMarket (proxy) at address", proofMarketAddress);

        // Get the current assessor image ID and guest URL
        ProofMarket market = ProofMarket(proofMarketAddress);
        (bytes32 imageID, string memory guestUrl) = market.imageInfo();

        vm.startBroadcast(deployerAddress());
        // Upgrade the proxy contract and update assessor image info if needed
        bytes32 assessorImageId = deploymentConfig.assessorImageId;
        string memory assessorGuestUrl = deploymentConfig.assessorGuestUrl;
        if (assessorImageId != imageID || keccak256(bytes(assessorGuestUrl)) == keccak256(bytes(guestUrl))) {
            // TODO: Switch to using the Upgrades library once the development is more stable.
            UnsafeUpgrades.upgradeProxy(
                proofMarketAddress,
                newImplementation,
                abi.encodeCall(ProofMarket.upgrade, (assessorImageId, assessorGuestUrl)),
                proofMarketOwner
            );
        } else {
            // TODO: Switch to using the Upgrades library once the development is more stable.
            UnsafeUpgrades.upgradeProxy(proofMarketAddress, newImplementation, "", proofMarketOwner);
        }
        vm.stopBroadcast();

        console2.log("Upgraded ProofMarket (proxy) contract at", proofMarketAddress);
    }
}
