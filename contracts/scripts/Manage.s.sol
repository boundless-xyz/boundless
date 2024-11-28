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
import {RiscZeroSetVerifier} from "risc0/RiscZeroSetVerifier.sol";
import {BoundlessMarket} from "../src/BoundlessMarket.sol";
import {ConfigLoader, DeploymentConfig, ConfigParser} from "./Config.s.sol";
import {UnsafeUpgrades, Upgrades} from "openzeppelin-foundry-upgrades/Upgrades.sol";

/// @notice Base contract for the scripts below, providing common context and functions.
contract RiscZeroManagementScript is Script {
    // Path to deployment config file, relative to the project root.
    string constant CONFIG = "contracts/deployment.toml";

    /// @notice Returns the address of the deployer, set in the DEPLOYER_ADDRESS env var.
    function deployerAddress() internal returns (address) {
        address deployer = vm.envAddress("DEPLOYER_ADDRESS");
        uint256 deployerKey = vm.envOr("DEPLOYER_PRIVATE_KEY", uint256(0));
        if (deployerKey != 0) {
            require(vm.addr(deployerKey) == deployer, "DEPLOYER_ADDRESS and DEPLOYER_PRIVATE_KEY are inconsistent");
            vm.rememberKey(deployerKey);
        }
        return deployer;
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

        address verifier = deploymentConfig.verifier;
        require(verifier != address(0), "verifier address must be set in config");
        console2.log("Using IRiscZeroVerifier at address", verifier);
        bytes32 assessorImageId = deploymentConfig.assessorImageId;
        require(assessorImageId != bytes32(0), "Assessor image ID must be set in config");
        string memory assessorGuestUrl = deploymentConfig.assessorGuestUrl;
        require(bytes(assessorGuestUrl).length != 0, "Assessor guest URL must be set in config");
        console2.log("Assessor info:");
        console2.log("image ID:", Strings.toHexString(uint256(assessorImageId)));
        console2.log("URL:", assessorGuestUrl);

        vm.broadcast(deployerAddress());
        // Deploy the market implementation
        address newImplementation = address(new BoundlessMarket(IRiscZeroVerifier(verifier), assessorImageId));
        console2.log("Deployed new BoundlessMarket implementation at", newImplementation);

        vm.broadcast(deployerAddress());
        // Deploy the proxy contract and initialize the contract
        // TODO(#108): Switch to using the Upgrades library.
        address marketAddress = UnsafeUpgrades.deployUUPSProxy(
            newImplementation, abi.encodeCall(BoundlessMarket.initialize, (marketOwner, assessorGuestUrl))
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
        address marketAddress = deploymentConfig.boundlessMarket;
        require(marketAddress != address(0), "BoundlessMarket (proxy) address must be set in config");
        console2.log("Using BoundlessMarket (proxy) at address", marketAddress);

        // Get the current assessor image ID and guest URL
        BoundlessMarket market = BoundlessMarket(marketAddress);
        (bytes32 imageID, string memory guestUrl) = market.imageInfo();

        // Use the same verifier as the existing implementation.
        IRiscZeroVerifier verifier = market.VERIFIER();
        bytes32 assessorImageId = deploymentConfig.assessorImageId;

        vm.startBroadcast(deployerAddress());

        // Deploy the market implementation
        address newImplementation = address(new BoundlessMarket(verifier, assessorImageId));
        console2.log("Deployed new BoundlessMarket implementation at", newImplementation);

        // Upgrade the proxy contract and update assessor image info if needed
        string memory assessorGuestUrl = deploymentConfig.assessorGuestUrl;
        if (assessorImageId != imageID || keccak256(bytes(assessorGuestUrl)) == keccak256(bytes(guestUrl))) {
            // TODO(#108): Switch to using the Upgrades library.
            UnsafeUpgrades.upgradeProxy(
                marketAddress,
                newImplementation,
                abi.encodeCall(BoundlessMarket.setImageUrl, (assessorGuestUrl)),
                marketOwner
            );
        } else {
            // TODO(#108): Switch to using the Upgrades library.
            UnsafeUpgrades.upgradeProxy(marketAddress, newImplementation, "", marketOwner);
        }
        vm.stopBroadcast();

        console2.log("Upgraded BoundlessMarket (proxy) contract at", marketAddress);
    }
}
