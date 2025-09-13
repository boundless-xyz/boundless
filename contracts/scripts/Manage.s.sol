// Copyright 2025 RISC Zero, Inc.
//
// Use of this source code is governed by the Business Source License
// as found in the LICENSE-BSL file.

pragma solidity ^0.8.9;

import {Script} from "forge-std/Script.sol";
import {console2} from "forge-std/console2.sol";
import {Strings} from "openzeppelin/contracts/utils/Strings.sol";
import {IRiscZeroVerifier} from "risc0/IRiscZeroVerifier.sol";
import {BoundlessMarket} from "../src/BoundlessMarket.sol";
import {BoundlessMarketLib} from "../src/libraries/BoundlessMarketLib.sol";
import {ConfigLoader, DeploymentConfig} from "./Config.s.sol";
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

// This is the EIP-1967 implementation slot:
bytes32 constant IMPLEMENTATION_SLOT = 0x360894A13BA1A3210667C828492DB98DCA3E2076CC3735A920A3CA505D382BBC;

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
        address collateralToken = deploymentConfig.collateralToken.required("collateral-token");

        vm.startBroadcast(deployerAddress());
        // Deploy the proxy contract and initialize the contract
        bytes32 salt = bytes32(0);
        address newImplementation = address(
            new BoundlessMarket{salt: salt}(
                IRiscZeroVerifier(verifier), assessorImageId, bytes32(0), 0, collateralToken
            )
        );
        address marketAddress = address(
            new ERC1967Proxy{salt: salt}(
                newImplementation, abi.encodeCall(BoundlessMarket.initialize, (admin, assessorGuestUrl))
            )
        );

        vm.stopBroadcast();

        // Verify the deployment
        BoundlessMarket market = BoundlessMarket(marketAddress);
        require(market.VERIFIER() == IRiscZeroVerifier(deploymentConfig.verifier), "verifier does not match");
        (bytes32 assessorId, string memory guestUrl) = market.imageInfo();
        require(assessorId == deploymentConfig.assessorImageId, "assessor image ID does not match");
        require(
            keccak256(bytes(guestUrl)) == keccak256(bytes(deploymentConfig.assessorGuestUrl)),
            "assessor guest URL does not match"
        );
        require(
            market.COLLATERAL_TOKEN_CONTRACT() == deploymentConfig.collateralToken, "collateral token does not match"
        );
        require(
            market.hasRole(market.ADMIN_ROLE(), deploymentConfig.admin), "market admin role does not match the admin"
        );

        console2.log("BoundlessMarket admin is %s", deploymentConfig.admin);
        console2.log("BoundlessMarket stake token contract at %s", deploymentConfig.collateralToken);
        console2.log("BoundlessMarket verifier contract at %s", deploymentConfig.verifier);
        console2.log("BoundlessMarket assessor image ID %s", Strings.toHexString(uint256(assessorId), 32));
        console2.log("BoundlessMarket assessor guest URL %s", guestUrl);

        address boundlessMarketImpl = address(uint160(uint256(vm.load(marketAddress, IMPLEMENTATION_SLOT))));
        console2.log(
            "Deployed BoundlessMarket proxy contract at %s with impl at %s", marketAddress, boundlessMarketImpl
        );

        string[] memory args = new string[](8);
        args[0] = "python3";
        args[1] = "contracts/update_deployment_toml.py";
        args[2] = "--boundless-market";
        args[3] = Strings.toHexString(marketAddress);
        args[4] = "--boundless-market-impl";
        args[5] = Strings.toHexString(boundlessMarketImpl);
        args[6] = "--boundless-market-old-impl";
        args[7] = Strings.toHexString(address(0)); // Old impl is not set at deployment time

        vm.ffi(args);
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
        address collateralToken = deploymentConfig.collateralToken.required("collateral-token");
        address verifier = deploymentConfig.verifier.required("verifier");
        address currentImplementation = address(uint160(uint256(vm.load(marketAddress, IMPLEMENTATION_SLOT))));
        uint32 deprecatedAssessorDuration = deploymentConfig.deprecatedAssessorDuration;

        // Get the current assessor image ID and guest URL
        BoundlessMarket market = BoundlessMarket(marketAddress);
        (bytes32 deprecatedAssessorImageId, string memory deprecatedGuestUrl) = market.imageInfo();

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
        opts.constructorData = BoundlessMarketLib.encodeConstructorArgs(
            IRiscZeroVerifier(verifier),
            assessorImageId,
            deprecatedAssessorImageId,
            deprecatedAssessorDuration,
            collateralToken
        );
        opts.referenceContract = "build-info-reference:BoundlessMarket";
        opts.referenceBuildInfoDir = "contracts/build-info-reference";

        // Upgrade the proxy contract and update assessor image info if needed.
        // Otherwise, we don't include it to save gas.
        vm.startBroadcast(admin);
        if (
            assessorImageId != deprecatedAssessorImageId
                || keccak256(bytes(assessorGuestUrl)) != keccak256(bytes(deprecatedGuestUrl))
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

        // Verify the upgrade
        BoundlessMarket upgradedMarket = BoundlessMarket(marketAddress);
        require(
            upgradedMarket.VERIFIER() == IRiscZeroVerifier(deploymentConfig.verifier),
            "upgraded market verifier does not match"
        );
        (bytes32 assessorId, string memory upgradedGuestUrl) = upgradedMarket.imageInfo();
        require(assessorId == deploymentConfig.assessorImageId, "upgraded market assessor image ID does not match");
        require(
            keccak256(bytes(upgradedGuestUrl)) == keccak256(bytes(deploymentConfig.assessorGuestUrl)),
            "upgraded market assessor guest URL does not match"
        );
        require(
            upgradedMarket.COLLATERAL_TOKEN_CONTRACT() == deploymentConfig.collateralToken,
            "upgraded market stake token does not match"
        );
        require(
            upgradedMarket.hasRole(upgradedMarket.ADMIN_ROLE(), deploymentConfig.admin),
            "upgraded market admin does not match the admin"
        );

        address boundlessMarketImpl = address(uint160(uint256(vm.load(marketAddress, IMPLEMENTATION_SLOT))));

        console2.log("Upgraded BoundlessMarket admin is %s", deploymentConfig.admin);
        console2.log("Upgraded BoundlessMarket proxy contract at %s", marketAddress);
        console2.log("Upgraded BoundlessMarket impl contract at %s", boundlessMarketImpl);
        console2.log("Upgraded BoundlessMarket stake token contract at %s", deploymentConfig.collateralToken);
        console2.log("Upgraded BoundlessMarket verifier contract at %s", deploymentConfig.verifier);
        console2.log("Upgraded BoundlessMarket assessor image ID %s", Strings.toHexString(uint256(assessorId), 32));
        console2.log("Upgraded BoundlessMarket assessor guest URL %s", upgradedGuestUrl);

        string[] memory args = new string[](6);
        args[0] = "python3";
        args[1] = "contracts/update_deployment_toml.py";
        args[2] = "--boundless-market-impl";
        args[3] = Strings.toHexString(boundlessMarketImpl);
        args[4] = "--boundless-market-old-impl";
        args[5] = Strings.toHexString(currentImplementation);

        vm.ffi(args);
    }
}

/// @notice Deployment script for the market contract rollback.
/// @dev Set values in deployment.toml to configure the deployment.
contract RollbackBoundlessMarket is BoundlessScript {
    function run() external {
        // Load the config
        DeploymentConfig memory deploymentConfig =
            ConfigLoader.loadDeploymentConfig(string.concat(vm.projectRoot(), "/", CONFIG));

        address admin = deploymentConfig.admin.required("admin");
        address marketAddress = deploymentConfig.boundlessMarket.required("boundless-market");
        string memory assessorGuestUrl = deploymentConfig.assessorGuestUrl.required("assessor-guest-url");
        address oldImplementation = deploymentConfig.boundlessMarketOldImpl.required("boundless-market-old-impl");

        require(oldImplementation != address(0), "old implementation address is not set");
        console2.log(
            "\nWARNING: This will rollback the BoundlessMarket contract to this address: %s\n", oldImplementation
        );

        // Rollback the proxy contract.
        vm.startBroadcast(admin);

        bytes memory initializer = abi.encodeCall(BoundlessMarket.setImageUrl, (assessorGuestUrl));
        bytes memory rollbackUpgradeData =
            abi.encodeWithSignature("upgradeToAndCall(address,bytes)", oldImplementation, initializer);

        (bool success, bytes memory returnData) = marketAddress.call(rollbackUpgradeData);
        require(success, string(returnData));

        vm.stopBroadcast();

        // Verify the upgrade
        BoundlessMarket upgradedMarket = BoundlessMarket(marketAddress);
        require(
            upgradedMarket.VERIFIER() == IRiscZeroVerifier(deploymentConfig.verifier),
            "upgraded market verifier does not match"
        );
        (bytes32 assessorId, string memory upgradedGuestUrl) = upgradedMarket.imageInfo();
        require(assessorId == deploymentConfig.assessorImageId, "upgraded market assessor image ID does not match");
        require(
            keccak256(bytes(upgradedGuestUrl)) == keccak256(bytes(deploymentConfig.assessorGuestUrl)),
            "upgraded market assessor guest URL does not match"
        );
        require(
            upgradedMarket.COLLATERAL_TOKEN_CONTRACT() == deploymentConfig.collateralToken,
            "upgraded market stake token does not match"
        );
        require(
            upgradedMarket.hasRole(upgradedMarket.ADMIN_ROLE(), deploymentConfig.admin),
            "upgraded market admin does not match the admin"
        );

        console2.log("Upgraded BoundlessMarket admin is %s", deploymentConfig.admin);
        console2.log("Upgraded BoundlessMarket proxy contract at %s", marketAddress);
        console2.log("Upgraded BoundlessMarket stake token contract at %s", deploymentConfig.collateralToken);
        console2.log("Upgraded BoundlessMarket verifier contract at %s", deploymentConfig.verifier);
        console2.log("Upgraded BoundlessMarket assessor image ID %s", Strings.toHexString(uint256(assessorId), 32));
        console2.log("Upgraded BoundlessMarket assessor guest URL %s", upgradedGuestUrl);

        address currentImplementation = address(uint160(uint256(vm.load(marketAddress, IMPLEMENTATION_SLOT))));
        require(
            currentImplementation == oldImplementation,
            "current implementation address does not match the old implementation address"
        );
        console2.log("Rollback successful. Current implementation address is now %s", currentImplementation);

        string[] memory args = new string[](4);
        args[0] = "python3";
        args[1] = "contracts/update_deployment_toml.py";
        args[2] = "--boundless-market-impl";
        args[3] = Strings.toHexString(currentImplementation);

        vm.ffi(args);
    }
}

/// @notice Script for granting admin role to a new address on the BoundlessMarket contract.
/// @dev Grants ADMIN_ROLE to the admin address set in deployment.toml
///
/// See the Foundry documentation for more information about Solidity scripts.
/// https://book.getfoundry.sh/tutorials/solidity-scripting
contract GrantAdminRole is BoundlessScript {
    function run() external {
        // Load the config
        DeploymentConfig memory deploymentConfig =
            ConfigLoader.loadDeploymentConfig(string.concat(vm.projectRoot(), "/", CONFIG));

        address newAdmin = deploymentConfig.admin.required("admin");
        address marketAddress = deploymentConfig.boundlessMarket.required("boundless-market");
        BoundlessMarket market = BoundlessMarket(marketAddress);

        require(!market.hasRole(market.ADMIN_ROLE(), newAdmin), "address already has admin role");

        // Get current admin with ADMIN_ROLE - use deployer as they should have admin role
        address currentAdmin = deployerAddress();
        require(market.hasRole(market.ADMIN_ROLE(), currentAdmin), "deployer does not have admin role");

        vm.broadcast(currentAdmin);
        market.grantRole(market.ADMIN_ROLE(), newAdmin);

        console2.log("Granted admin role on the BoundlessMarket contract to %s", newAdmin);
    }
}
