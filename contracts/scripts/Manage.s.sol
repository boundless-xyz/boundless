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
import {BoundlessMarket} from "../src/BoundlessMarket.sol";
import {BoundlessMarketLib} from "../src/BoundlessMarketLib.sol";
import {ConfigLoader, DeploymentConfig, ConfigParser} from "./Config.s.sol";
import {Upgrades} from "openzeppelin-foundry-upgrades/Upgrades.sol";
import {Options as UpgradeOptions} from "openzeppelin-foundry-upgrades/Options.sol";

/// @notice Base contract for the scripts below, providing common context and functions.
contract RiscZeroManagementScript is Script {
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

/// @notice Deployment script for the market deployment.
/// @dev Use the following environment variable to control the deployment:
///     * BOUNDLESS_MARKET_OWNER owner of the BoundlessMarket contract
///
/// See the Foundry documentation for more information about Solidity scripts.
/// https://book.getfoundry.sh/tutorials/solidity-scripting
contract DeployBoundlessMarket is RiscZeroManagementScript {
    function run() external {
        address marketOwner = vm.envAddress("BOUNDLESS_MARKET_OWNER");
        console2.log("marketOwner:", marketOwner);

        // Load the config
        DeploymentConfig memory deploymentConfig =
            ConfigLoader.loadDeploymentConfig(string.concat(vm.projectRoot(), "/", CONFIG));

        address router = deploymentConfig.router;
        require(router != address(0), "RiscZeroVerifierRouter address must be set in config");
        console2.log("Using RiscZeroVerifierRouter at address", router);
        bytes32 assessorImageId = deploymentConfig.assessorImageId;
        require(assessorImageId != bytes32(0), "Assessor image ID must be set in config");
        string memory assessorGuestUrl = deploymentConfig.assessorGuestUrl;
        require(bytes(assessorGuestUrl).length != 0, "Assessor guest URL must be set in config");
        console2.log("Assessor info:");
        console2.logBytes32(assessorImageId);
        console2.logString(assessorGuestUrl);

        UpgradeOptions memory opts;
        opts.constructorData = BoundlessMarketLib.encodeConstructorArgs(IRiscZeroVerifier(router), assessorImageId);

        vm.broadcast(deployerAddress());
        // Deploy the proxy contract and initialize the contract
        address boundlessMarketAddress = Upgrades.deployUUPSProxy(
            "BoundlessMarket.sol:BoundlessMarket",
            abi.encodeCall(BoundlessMarket.initialize, (marketOwner, assessorGuestUrl)),
            opts
        );

        console2.log("Deployed BoundlessMarket (proxy) contract at", marketAddress);
    }
}

/// @notice Deployment script for the market contract upgrade.
/// @dev Use the following environment variable to control the deployment:
///     * BOUNDLESS_MARKET_OWNER owner of the BoundlessMarket contract
///
/// See the Foundry documentation for more information about Solidity scripts.
/// https://book.getfoundry.sh/tutorials/solidity-scripting
contract UpgradeBoundlessMarket is RiscZeroManagementScript {
    function run() external {
        address marketOwner = vm.envAddress("BOUNDLESS_MARKET_OWNER");
        console2.log("marketOwner:", marketOwner);

        // Load the config
        DeploymentConfig memory deploymentConfig =
            ConfigLoader.loadDeploymentConfig(string.concat(vm.projectRoot(), "/", CONFIG));
        address marketAddress = deploymentConfig.market;
        require(marketAddress != address(0), "BoundlessMarket (proxy) address must be set in config");
        console2.log("Using BoundlessMarket (proxy) at address", marketAddress);

        // Get the current assessor image ID and guest URL
        BoundlessMarket market = BoundlessMarket(marketAddress);
        (bytes32 imageID, string memory guestUrl) = market.imageInfo();

        // Use the same verifier as the existing implementation.
        IRiscZeroVerifier verifier = market.VERIFIER();
        bytes32 assessorImageId = deploymentConfig.assessorImageId;

        UpgradeOptions memory opts;
        opts.constructorData = ProofMarketLib.encodeConstructorArgs(verifier, assessorImageId);
        opts.referenceContract = "build-info-reference:ProofMarket";
        opts.referenceBuildInfoDir = "contracts/reference-contract/build-info-reference";

        vm.startBroadcast(deployerAddress());
        // Upgrade the proxy contract and update assessor image info if needed
        string memory assessorGuestUrl = deploymentConfig.assessorGuestUrl;
        if (assessorImageId != imageID || keccak256(bytes(assessorGuestUrl)) == keccak256(bytes(guestUrl))) {
            Upgrades.upgradeProxy(
                marketAddress,
                "BoundlessMarket.sol:BoundlessMarket",
                abi.encodeCall(BoundlessMarket.setImageUrl, (assessorGuestUrl)),
                opts,
                marketOwner
            );
        } else {
            Upgrades.upgradeProxy(proofMarketAddress, "BoundlessMarket.sol:BoundlessMarket", "", opts, marketOwner);
        }
        vm.stopBroadcast();

        console2.log("Upgraded BoundlessMarket (proxy) contract at", marketAddress);
    }
}
