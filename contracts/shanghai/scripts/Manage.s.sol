// Copyright 2026 Boundless Foundation, Inc.
//
// Use of this source code is governed by the Business Source License
// as found in the LICENSE-BSL file.

pragma solidity ^0.8.26;

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
import {BoundlessScriptBase} from "./BoundlessScript.s.sol";

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

/// @notice Deployment script for the market deployment.
/// @dev Set values in deployment.toml to configure the deployment.
///
/// See the Foundry documentation for more information about Solidity scripts.
/// https://book.getfoundry.sh/tutorials/solidity-scripting
contract DeployBoundlessMarket is BoundlessScriptBase {
    function run() external {
        // Load the config
        DeploymentConfig memory deploymentConfig =
            ConfigLoader.loadDeploymentConfig(string.concat(vm.projectRoot(), "/", CONFIG));

        address admin = deploymentConfig.admin.required("admin");
        address verifier = deploymentConfig.verifier.required("verifier");
        address applicationVerifier = deploymentConfig.applicationVerifier.required("application-verifier");
        bytes32 assessorImageId = deploymentConfig.assessorImageId.required("assessor-image-id");
        string memory assessorGuestUrl = deploymentConfig.assessorGuestUrl.required("assessor-guest-url");
        address collateralToken = deploymentConfig.collateralToken.required("collateral-token");

        vm.startBroadcast(getDeployer());
        // Deploy the proxy contract and initialize the contract
        bytes32 salt = bytes32(0);
        address newImplementation = address(
            new BoundlessMarket{salt: salt}(
                IRiscZeroVerifier(verifier),
                IRiscZeroVerifier(applicationVerifier),
                assessorImageId,
                bytes32(0),
                0,
                collateralToken
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
        require(
            market.APPLICATION_VERIFIER() == IRiscZeroVerifier(deploymentConfig.applicationVerifier),
            "application verifier does not match"
        );
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
        console2.log("BoundlessMarket application verifier contract at %s", deploymentConfig.applicationVerifier);
        console2.log("BoundlessMarket assessor image ID %s", Strings.toHexString(uint256(assessorId), 32));
        console2.log("BoundlessMarket assessor guest URL %s", guestUrl);

        address boundlessMarketImpl = address(uint160(uint256(vm.load(marketAddress, IMPLEMENTATION_SLOT))));
        console2.log(
            "Deployed BoundlessMarket proxy contract at %s with impl at %s", marketAddress, boundlessMarketImpl
        );

        // Get current git commit hash
        string memory currentCommit = getCurrentCommit();

        string[] memory args = new string[](10);
        args[0] = "python3";
        args[1] = "contracts/update_deployment_toml.py";
        args[2] = "--boundless-market";
        args[3] = Strings.toHexString(marketAddress);
        args[4] = "--boundless-market-impl";
        args[5] = Strings.toHexString(boundlessMarketImpl);
        args[6] = "--boundless-market-old-impl";
        args[7] = Strings.toHexString(address(0)); // Old impl is not set at deployment time
        args[8] = "--boundless-market-deployment-commit";
        args[9] = currentCommit;

        vm.ffi(args);
        console2.log("Updated BoundlessMarket deployment commit: %s", currentCommit);

        // Check for uncommitted changes warning
        checkUncommittedChangesWarning("Deployment");
    }
}

/// @notice Deployment script for the market contract upgrade.
/// @dev Set values in deployment.toml to configure the deployment.
///
/// See the Foundry documentation for more information about Solidity scripts.
/// https://book.getfoundry.sh/tutorials/solidity-scripting
contract UpgradeBoundlessMarket is BoundlessScriptBase {
    function run() external {
        // Check for deployment mode flags
        bool gnosisExecute = vm.envOr("GNOSIS_EXECUTE", false);
        bool skipSafetyChecks = vm.envOr("SKIP_SAFETY_CHECKS", false);

        // Load the config
        DeploymentConfig memory deploymentConfig =
            ConfigLoader.loadDeploymentConfig(string.concat(vm.projectRoot(), "/", CONFIG));

        address marketAddress = deploymentConfig.boundlessMarket.required("boundless-market");
        address collateralToken = deploymentConfig.collateralToken.required("collateral-token");
        address verifier = deploymentConfig.verifier.required("verifier");
        address applicationVerifier = deploymentConfig.applicationVerifier.required("application-verifier");
        address currentImplementation = address(uint160(uint256(vm.load(marketAddress, IMPLEMENTATION_SLOT))));
        uint32 deprecatedAssessorDuration = deploymentConfig.deprecatedAssessorDuration;

        // Get the current assessor image ID and guest URL
        BoundlessMarket market = BoundlessMarket(marketAddress);
        (bytes32 deprecatedAssessorImageId,) = market.imageInfo();

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
            IRiscZeroVerifier(applicationVerifier),
            assessorImageId,
            deprecatedAssessorImageId,
            deprecatedAssessorDuration,
            collateralToken
        );

        if (skipSafetyChecks) {
            console2.log("WARNING: Skipping all upgrade safety checks and reference build!");
            opts.unsafeSkipAllChecks = true;
        } else {
            // Only set reference contract when doing safety checks
            opts.referenceContract = "build-info-reference:BoundlessMarket";
            opts.referenceBuildInfoDir = "contracts/build-info-reference";
        }

        address newImpl = address(0);
        bytes memory initializerData = abi.encodeCall(BoundlessMarket.setImageUrl, (assessorGuestUrl));

        vm.startBroadcast(getDeployer());
        if (gnosisExecute) {
            console2.log("GNOSIS_EXECUTE=true: Deploying new implementation for Safe upgrade");
            console2.log("Target proxy address: ", marketAddress);
            console2.log("Current implementation: ", currentImplementation);

            // Use prepareUpgrade for validation + deployment
            newImpl = Upgrades.prepareUpgrade("BoundlessMarket.sol:BoundlessMarket", opts);
            console2.log("New implementation deployed: ", newImpl);

            // Print Gnosis Safe transaction info
            _printGnosisSafeInfo(marketAddress, newImpl, initializerData);
        } else {
            console2.log("Upgrading Boundless Market at: ", marketAddress);
            console2.log("Current implementation: ", currentImplementation);

            // Perform upgrade with optional initializer
            if (initializerData.length > 0) {
                Upgrades.upgradeProxy(marketAddress, "BoundlessMarket.sol:BoundlessMarket", initializerData, opts);
            } else {
                Upgrades.upgradeProxy(marketAddress, "BoundlessMarket.sol:BoundlessMarket", "", opts);
            }

            newImpl = Upgrades.getImplementationAddress(marketAddress);
            console2.log("Upgraded Boundless Market implementation to: ", newImpl);

            // Verify the upgrade
            BoundlessMarket upgradedMarket = BoundlessMarket(marketAddress);
            require(
                upgradedMarket.VERIFIER() == IRiscZeroVerifier(deploymentConfig.verifier),
                "upgraded market verifier does not match"
            );
            require(
                upgradedMarket.APPLICATION_VERIFIER() == IRiscZeroVerifier(deploymentConfig.applicationVerifier),
                "upgraded market application verifier does not match"
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
                upgradedMarket.hasRole(upgradedMarket.ADMIN_ROLE(), deploymentConfig.admin2),
                "upgraded market admin does not match the admin"
            );
            address boundlessMarketImpl = address(uint160(uint256(vm.load(marketAddress, IMPLEMENTATION_SLOT))));
            console2.log("Upgraded BoundlessMarket admin is %s", deploymentConfig.admin);
            console2.log("Upgraded BoundlessMarket proxy contract at %s", marketAddress);
            console2.log("Upgraded BoundlessMarket impl contract at %s", boundlessMarketImpl);
            console2.log("Upgraded BoundlessMarket collateral token contract at %s", deploymentConfig.collateralToken);
            console2.log("Upgraded BoundlessMarket verifier contract at %s", deploymentConfig.verifier);
            console2.log(
                "Upgraded BoundlessMarket application verifier contract at %s", deploymentConfig.applicationVerifier
            );
            console2.log("Upgraded BoundlessMarket assessor image ID %s", Strings.toHexString(uint256(assessorId), 32));
            console2.log("Upgraded BoundlessMarket assessor guest URL %s", upgradedGuestUrl);
        }
        vm.stopBroadcast();

        string[] memory args = new string[](6);
        args[0] = "python3";
        args[1] = "contracts/update_deployment_toml.py";
        args[2] = "--boundless-market-impl";
        args[3] = Strings.toHexString(newImpl);
        args[4] = "--boundless-market-old-impl";
        args[5] = Strings.toHexString(currentImplementation);

        vm.ffi(args);
    }
}

/// @notice Deployment script for the market contract rollback.
/// @dev Set values in deployment.toml to configure the deployment.
contract RollbackBoundlessMarket is BoundlessScriptBase {
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
        require(
            upgradedMarket.APPLICATION_VERIFIER() == IRiscZeroVerifier(deploymentConfig.applicationVerifier),
            "upgraded market application verifier does not match"
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
        console2.log("Upgraded BoundlessMarket collateral token contract at %s", deploymentConfig.collateralToken);
        console2.log("Upgraded BoundlessMarket verifier contract at %s", deploymentConfig.verifier);
        console2.log(
            "Upgraded BoundlessMarket application verifier contract at %s", deploymentConfig.applicationVerifier
        );
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

/// @notice Script for adding admin role to a new address on the BoundlessMarket contract.
/// @dev Grants ADMIN_ROLE to the address specified in ADMIN_TO_ADD environment variable
///
/// Sample Usage:
/// export CHAIN_KEY="anvil"
/// export ADMIN_TO_ADD="0x70997970C51812dc3A010C7d01b50e0d17dc79C8"
/// forge script contracts/scripts/Manage.s.sol:AddBoundlessMarketAdmin \
///     --private-key <PRIVATE_KEY> \
///     --broadcast \
///     --rpc-url <RPC_URL>
contract AddBoundlessMarketAdmin is BoundlessScriptBase {
    function run() external {
        // Load the config
        DeploymentConfig memory deploymentConfig =
            ConfigLoader.loadDeploymentConfig(string.concat(vm.projectRoot(), "/", CONFIG));

        address adminToAdd = vm.envAddress("ADMIN_TO_ADD");
        require(adminToAdd != address(0), "ADMIN_TO_ADD environment variable not set");

        address marketAddress = deploymentConfig.boundlessMarket.required("boundless-market");
        BoundlessMarket market = BoundlessMarket(marketAddress);

        bool gnosisExecute = vm.envOr("GNOSIS_EXECUTE", false);
        bytes32 adminRole = market.ADMIN_ROLE();

        if (gnosisExecute) {
            console2.log("GNOSIS_EXECUTE=true: Preparing grantRole calldata for Safe execution");
            console2.log("BoundlessMarket Contract: ", marketAddress);
            console2.log("Admin to Add: ", adminToAdd);
            console2.log("Role: ADMIN_ROLE");

            // Print Gnosis Safe transaction info for grantRole
            bytes memory grantRoleCallData =
                abi.encodeWithSignature("grantRole(bytes32,address)", adminRole, adminToAdd);
            console2.log("================================");
            console2.log("=== GNOSIS SAFE GRANT ROLE INFO ===");
            console2.log("Target Address (To): ", marketAddress);
            console2.log("Function: grantRole(bytes32,address)");
            console2.log("Role: ");
            console2.logBytes32(adminRole);
            console2.log("Account: ", adminToAdd);
            console2.log("Calldata:");
            console2.logBytes(grantRoleCallData);
            console2.log("=====================================");
            console2.log("BoundlessMarket Admin Grant Role Calldata Ready");
            console2.log("Transaction NOT executed - use Gnosis Safe to execute");
        } else {
            // Get current admin with ADMIN_ROLE - use deployer as they should have admin role
            address currentAdmin = getDeployer();
            require(market.hasRole(market.ADMIN_ROLE(), currentAdmin), "deployer does not have admin role");

            vm.broadcast(currentAdmin);
            market.grantRole(adminRole, adminToAdd);

            // Sanity checks
            console2.log("BoundlessMarket Contract: ", marketAddress);
            console2.log("New BoundlessMarket Admin: ", adminToAdd);
            console2.log("ADMIN_ROLE granted: ", market.hasRole(adminRole, adminToAdd));
            console2.log("Other admin: ", currentAdmin);
            console2.log("Other admin still active: ", market.hasRole(adminRole, currentAdmin));
            console2.log("================================================");
            console2.log("BoundlessMarket Admin Role Updated Successfully");
        }

        _updateDeploymentConfig("admin-2", adminToAdd);
    }
}

/// @notice Script for removing admin role from an address on the BoundlessMarket contract.
/// @dev Revokes ADMIN_ROLE from the address specified in ADMIN_TO_REMOVE environment variable
///
/// Sample Usage:
/// export CHAIN_KEY="anvil"
/// export ADMIN_TO_REMOVE="0x70997970C51812dc3A010C7d01b50e0d17dc79C8"
/// forge script contracts/scripts/Manage.s.sol:RemoveBoundlessMarketAdmin \
///     --private-key <PRIVATE_KEY> \
///     --broadcast \
///     --rpc-url <RPC_URL>
contract RemoveBoundlessMarketAdmin is BoundlessScriptBase {
    function run() external {
        // Load the config
        DeploymentConfig memory deploymentConfig =
            ConfigLoader.loadDeploymentConfig(string.concat(vm.projectRoot(), "/", CONFIG));

        address adminToRemove = vm.envAddress("ADMIN_TO_REMOVE");
        require(adminToRemove != address(0), "ADMIN_TO_REMOVE environment variable not set");

        address marketAddress = deploymentConfig.boundlessMarket.required("boundless-market");
        BoundlessMarket market = BoundlessMarket(marketAddress);

        bool gnosisExecute = vm.envOr("GNOSIS_EXECUTE", false);
        bytes32 adminRole = market.ADMIN_ROLE();

        // Safety check: Ensure at least one other admin will remain
        address otherAdmin =
            (adminToRemove == deploymentConfig.admin) ? deploymentConfig.admin2 : deploymentConfig.admin;

        require(
            otherAdmin != address(0) && market.hasRole(adminRole, otherAdmin),
            "Cannot remove admin: would leave BoundlessMarket without any admins"
        );

        if (gnosisExecute) {
            console2.log("GNOSIS_EXECUTE=true: Preparing revokeRole calldata for Safe execution");
            console2.log("BoundlessMarket Contract: ", marketAddress);
            console2.log("Admin to Remove: ", adminToRemove);
            console2.log("Role: ADMIN_ROLE");

            // Print Gnosis Safe transaction info for revokeRole
            bytes memory revokeRoleCallData =
                abi.encodeWithSignature("revokeRole(bytes32,address)", adminRole, adminToRemove);
            console2.log("================================");
            console2.log("=== GNOSIS SAFE REVOKE ROLE INFO ===");
            console2.log("Target Address (To): ", marketAddress);
            console2.log("Function: revokeRole(bytes32,address)");
            console2.log("Role: ");
            console2.logBytes32(adminRole);
            console2.log("Account: ", adminToRemove);
            console2.log("Calldata:");
            console2.logBytes(revokeRoleCallData);
            console2.log("=====================================");
            console2.log("BoundlessMarket Admin Revoke Role Calldata Ready");
            console2.log("Transaction NOT executed - use Gnosis Safe to execute");
        } else {
            // Get current admin with ADMIN_ROLE - use deployer as they should have admin role
            address currentAdmin = getDeployer();
            require(market.hasRole(market.ADMIN_ROLE(), currentAdmin), "deployer does not have admin role");

            vm.broadcast(currentAdmin);
            market.revokeRole(adminRole, adminToRemove);

            // Sanity checks
            console2.log("BoundlessMarket Contract: ", marketAddress);
            console2.log("Removed BoundlessMarket Admin: ", adminToRemove);
            console2.log("Other admin: ", otherAdmin);
            console2.log("Other Admin still active: ", market.hasRole(adminRole, otherAdmin));
            console2.log("ADMIN_ROLE revoked: ", !market.hasRole(adminRole, adminToRemove));
            console2.log("================================================");
            console2.log("BoundlessMarket Admin Role Removed Successfully");
        }

        _removeAdminFromToml("admin", "admin-2", adminToRemove);
    }
}
