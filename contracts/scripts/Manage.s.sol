// Copyright 2025 RISC Zero, Inc.
//
// Use of this source code is governed by the Business Source License
// as found in the LICENSE-BSL file.

pragma solidity ^0.8.9;

import {Script} from "forge-std/Script.sol";
import {console2} from "forge-std/console2.sol";
import {Strings} from "openzeppelin/contracts/utils/Strings.sol";
import {RiscZeroVerifierRouter} from "risc0/RiscZeroVerifierRouter.sol";
import {RiscZeroVerifierEmergencyStop} from "risc0/RiscZeroVerifierEmergencyStop.sol";
import {IRiscZeroVerifier} from "risc0/IRiscZeroVerifier.sol";
import {RiscZeroSetVerifier} from "risc0/RiscZeroSetVerifier.sol";
import {BoundlessMarket} from "../src/BoundlessMarket.sol";
import {BoundlessMarketLib} from "../src/libraries/BoundlessMarketLib.sol";
import {ConfigLoader, DeploymentConfig, ConfigParser} from "./Config.s.sol";
import {ERC1967Proxy} from "@openzeppelin/contracts/proxy/ERC1967/ERC1967Proxy.sol";
import {Upgrades} from "openzeppelin-foundry-upgrades/Upgrades.sol";
import {Options as UpgradeOptions} from "openzeppelin-foundry-upgrades/Options.sol";

library RequireLib {
    function required(address value, string memory label) internal pure returns (address) {
        if (value == address(0)) {
            console2.log("address value %s is required", label);
            require(false, "required address value not set");
        }
        console2.log("Using %s = %s", label, value);
        return value;
    }

    function required(bytes32 value, string memory label) internal pure returns (bytes32) {
        if (value == bytes32(0)) {
            console2.log("bytes32 value %s is required", label);
            require(false, "required bytes32 value not set");
        }
        console2.log("Using %s = %x", label, uint256(value));
        return value;
    }

    function required(string memory value, string memory label) internal pure returns (string memory) {
        if (bytes(value).length == 0) {
            console2.log("string value %s is required", label);
            require(false, "required string value not set");
        }
        console2.log("Using %s = %s", label, value);
        return value;
    }
}

using RequireLib for address;
using RequireLib for string;
using RequireLib for bytes32;

/// @notice Base contract for the scripts below, providing common context and functions.
contract BoundlessScript is Script {
    // Path to deployment config file, relative to the project root.
    string constant CONFIG = "contracts/deployment.toml";

    /// @notice Returns the address of the deployer, set in the DEPLOYER_ADDRESS env var.
    function deployerAddress() internal returns (address deployer) {
        uint256 deployerKey = vm.envOr("DEPLOYER_PRIVATE_KEY", uint256(0));
        if (deployerKey != 0) {
            deployer = vm.envOr("DEPLOYER_ADDRESS", vm.addr(deployerKey));
            require(vm.addr(deployerKey) == deployer, "DEPLOYER_ADDRESS and DEPLOYER_PRIVATE_KEY are inconsistent");
            vm.rememberKey(deployerKey);
        } else {
            deployer = vm.envOr("DEPLOYER_ADDRESS", address(0));
            require(deployer != address(0), "env var DEPLOYER_ADDRESS or DEPLOYER_PRIVATE_KEY required");
        }
        return deployer;
    }
}

/// @notice Deployment script for the market deployment.
/// @dev Set values in deployment.toml to configure the deployment.
///
/// See the Foundry documentation for more information about Solidity scripts.
/// https://book.getfoundry.sh/tutorials/solidity-scripting
contract DeployBoundlessMarket is BoundlessScript {
    function run() external {
        // Load the config
        DeploymentConfig memory deploymentConfig =
            ConfigLoader.loadDeploymentConfig(string.concat(vm.projectRoot(), "/", CONFIG));

        address admin = deploymentConfig.admin.required("admin");
        address verifier = deploymentConfig.verifier.required("verifier");
        bytes32 assessorImageId = deploymentConfig.assessorImageId.required("assessor-image-id");
        string memory assessorGuestUrl = deploymentConfig.assessorGuestUrl.required("assessor-guest-url");
        address stakeToken = deploymentConfig.stakeToken.required("stake-token");

        vm.startBroadcast(deployerAddress());
        // Deploy the proxy contract and initialize the contract
        bytes32 salt = bytes32(0);
        address newImplementation =
            address(new BoundlessMarket{salt: salt}(IRiscZeroVerifier(verifier), assessorImageId, stakeToken));
        address marketAddress = address(
            new ERC1967Proxy{salt: salt}(
                newImplementation, abi.encodeCall(BoundlessMarket.initialize, (admin, assessorGuestUrl))
            )
        );

        vm.stopBroadcast();

        console2.log("Deployed BoundlessMarket proxy contract at %s", marketAddress);
    }
}

/// @notice Deployment script for the market contract upgrade.
/// @dev Set values in deployment.toml to configure the deployment.
///
/// See the Foundry documentation for more information about Solidity scripts.
/// https://book.getfoundry.sh/tutorials/solidity-scripting
contract UpgradeBoundlessMarket is BoundlessScript {
    function run() external {
        // Load the config
        DeploymentConfig memory deploymentConfig =
            ConfigLoader.loadDeploymentConfig(string.concat(vm.projectRoot(), "/", CONFIG));

        address admin = deploymentConfig.admin.required("admin");
        address marketAddress = deploymentConfig.boundlessMarket.required("boundless-market");
        address stakeToken = deploymentConfig.stakeToken.required("stake-token");
        address verifier = deploymentConfig.stakeToken.required("verifier");

        // Get the current assessor image ID and guest URL
        BoundlessMarket market = BoundlessMarket(marketAddress);
        (bytes32 currentImageID, string memory currentGuestUrl) = market.imageInfo();

        // Use the assessor image ID recorded in deployment.toml
        bytes32 assessorImageId = deploymentConfig.assessorImageId.required("assessor-image-id");
        string memory assessorGuestUrl = deploymentConfig.assessorGuestUrl.required("assessor-guest-url");

        // Upgrade requires build info from the currently deployed version.
        // You can get this build info with the following process.
        // Check the `deployment.toml` for the deployed commit.
        //
        // ```sh
        // git worktree add ../boundless-reference ${DEPLOYED_COMMIT:?}
        // cd ../boundless-reference
        // forge build
        // cp -R out/build-info ../boundless/contracts/build-info-reference
        // ```
        UpgradeOptions memory opts;
        opts.constructorData =
            BoundlessMarketLib.encodeConstructorArgs(IRiscZeroVerifier(verifier), assessorImageId, stakeToken);
        opts.referenceContract = "build-info-reference:BoundlessMarket";
        opts.referenceBuildInfoDir = "contracts/build-info-reference";

        // Upgrade the proxy contract and update assessor image info if needed.
        // Otherwise, we don't include it to save gas.
        vm.startBroadcast(admin);
        if (
            assessorImageId != currentImageID || keccak256(bytes(assessorGuestUrl)) != keccak256(bytes(currentGuestUrl))
        ) {
            Upgrades.upgradeProxy(
                marketAddress,
                "BoundlessMarket.sol:BoundlessMarket",
                abi.encodeCall(BoundlessMarket.setImageUrl, (assessorGuestUrl)),
                opts,
                admin
            );
        } else {
            Upgrades.upgradeProxy(marketAddress, "BoundlessMarket.sol:BoundlessMarket", "", opts, admin);
        }
        vm.stopBroadcast();

        console2.log("Upgraded BoundlessMarket proxy contract at %s", marketAddress);
    }
}

/// @notice Script from transferring ownership of the BoundlessMarket contract.
/// @dev Transfer will be from the current admin (i.e. owner) address to the admin address set in deployment.toml
///
/// See the Foundry documentation for more information about Solidity scripts.
/// https://book.getfoundry.sh/tutorials/solidity-scripting
contract TransferOwnership is BoundlessScript {
    function run() external {
        // Load the config
        DeploymentConfig memory deploymentConfig =
            ConfigLoader.loadDeploymentConfig(string.concat(vm.projectRoot(), "/", CONFIG));

        address admin = deploymentConfig.admin.required("admin");
        address marketAddress = deploymentConfig.boundlessMarket.required("boundless-market");
        BoundlessMarket market = BoundlessMarket(marketAddress);

        address currentAdmin = market.owner();
        require(admin != currentAdmin, "current and new admin address are the same");

        vm.broadcast(currentAdmin);
        market.transferOwnership(admin);

        console2.log("Transfered ownership of the BoundlessMarket contract from %s to %s", currentAdmin, admin);
        console2.log("Ownership must be accepted by the new admin %s", admin);
    }
}

/// @notice Script from accepting an ownership transfer of the BoundlessMarket contract.
///
/// See the Foundry documentation for more information about Solidity scripts.
/// https://book.getfoundry.sh/tutorials/solidity-scripting
contract AcceptTransferOwnership is BoundlessScript {
    function run() external {
        // Load the config
        DeploymentConfig memory deploymentConfig =
            ConfigLoader.loadDeploymentConfig(string.concat(vm.projectRoot(), "/", CONFIG));

        address admin = deploymentConfig.admin.required("admin");
        address marketAddress = deploymentConfig.boundlessMarket.required("boundless-market");
        BoundlessMarket market = BoundlessMarket(marketAddress);

        require(admin == market.pendingOwner(), "pending owner is not the configured admin");

        vm.broadcast(admin);
        market.acceptOwnership();

        console2.log("Accepted transfer of ownership of the BoundlessMarket contract from %s", admin);
    }
}
