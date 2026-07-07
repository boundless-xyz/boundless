// Copyright 2026 Boundless Foundation, Inc.
//
// Use of this source code is governed by the Business Source License
// as found in the LICENSE-BSL file.

pragma solidity ^0.8.26;

import {Script} from "forge-std/Script.sol";
import {console2} from "forge-std/console2.sol";
import {Strings} from "openzeppelin/contracts/utils/Strings.sol";
import {IRiscZeroVerifier} from "risc0/IRiscZeroVerifier.sol";
import {BoundlessMarket} from "boundless-market/BoundlessMarket.sol";
import {BoundlessMarket as BoundlessMarketLegacy} from "boundless-market-legacy/BoundlessMarketLegacy.sol";
import {BoundlessRouter} from "../src/router/BoundlessRouter.sol";
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
        address collateralToken = deploymentConfig.collateralToken.required("collateral-token");
        // Market dispatches verification via the BoundlessRouter, read from the
        // deployment config (the BOUNDLESS_ROUTER env var overrides it).
        address boundlessRouter =
            vm.envOr("BOUNDLESS_ROUTER", deploymentConfig.boundlessRouter).required("boundless-router");
        // Resolve the legacy impl (delegate-call target for the legacy ABI).
        // Production sets BOUNDLESS_LEGACY_IMPL to the audited on-chain impl;
        // when unset (dev / localnet / fresh networks) deploy a fresh one from
        // contracts/src/legacy/ wired to the configured verifier and token.
        address legacyImpl = vm.envOr("BOUNDLESS_LEGACY_IMPL", address(0));
        vm.startBroadcast(getDeployer());
        if (legacyImpl == address(0)) {
            legacyImpl = address(
                new BoundlessMarketLegacy(
                    IRiscZeroVerifier(deploymentConfig.verifier),
                    IRiscZeroVerifier(deploymentConfig.applicationVerifier),
                    deploymentConfig.assessorImageId,
                    bytes32(0),
                    0,
                    collateralToken
                )
            );
            console2.log("Deployed legacy BoundlessMarket implementation to", legacyImpl);
        } else {
            console2.log("Using BOUNDLESS_LEGACY_IMPL from env:", legacyImpl);
        }
        // Deploy the proxy contract and initialize the contract
        bytes32 salt = bytes32(0);
        address newImplementation =
            address(new BoundlessMarket{salt: salt}(BoundlessRouter(boundlessRouter), collateralToken, legacyImpl));
        address marketAddress = address(
            new ERC1967Proxy{salt: salt}(newImplementation, abi.encodeCall(BoundlessMarket.initialize, (admin)))
        );

        // Carry the legacy assessor guest URL onto the fresh proxy so old brokers — which
        // fetch the assessor guest from `imageInfo()` — keep working. Read from the market
        // this deployment replaces (deployment.toml's boundless-market; LEGACY_MARKET
        // overrides the address, LEGACY_ASSESSOR_GUEST_URL overrides the URL outright).
        ensureLegacyImageUrl(marketAddress, resolveLegacyImageUrl(deploymentConfig.boundlessMarket));

        vm.stopBroadcast();

        // Verify the deployment
        BoundlessMarket market = BoundlessMarket(payable(marketAddress));
        require(address(market.ROUTER()) == boundlessRouter, "router does not match");
        require(market.LEGACY_IMPL() == legacyImpl, "legacy impl does not match");
        require(
            market.COLLATERAL_TOKEN_CONTRACT() == deploymentConfig.collateralToken, "collateral token does not match"
        );
        require(
            market.hasRole(market.ADMIN_ROLE(), deploymentConfig.admin), "market admin role does not match the admin"
        );

        console2.log("BoundlessMarket admin is %s", deploymentConfig.admin);
        console2.log("BoundlessMarket stake token contract at %s", deploymentConfig.collateralToken);
        console2.log("BoundlessMarket router contract at %s", boundlessRouter);

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
        address currentImplementation = address(uint160(uint256(vm.load(marketAddress, IMPLEMENTATION_SLOT))));
        // Market now dispatches verification via the BoundlessRouter; the
        // pre-existing `verifier` / `applicationVerifier` / `assessorImageId` fields
        // are no longer market-level state. Read the router from the deployment
        // config (the BOUNDLESS_ROUTER env var overrides it).
        address boundlessRouter =
            vm.envOr("BOUNDLESS_ROUTER", deploymentConfig.boundlessRouter).required("boundless-router");
        // Keep the existing delegate-call target by default so the audited
        // legacy bytecode the proxy already points at is preserved across the
        // upgrade; BOUNDLESS_LEGACY_IMPL can override it to intentionally repoint.
        address legacyImpl = vm.envOr("BOUNDLESS_LEGACY_IMPL", address(0));
        if (legacyImpl == address(0)) {
            // A proxy already on the router-aware impl exposes LEGACY_IMPL(); a proxy still
            // on the legacy impl does not (no such getter, no fallback) — there the
            // implementation being replaced IS the legacy bytecode the fallback must keep
            // serving.
            (bool ok, bytes memory data) = marketAddress.staticcall(abi.encodeWithSignature("LEGACY_IMPL()"));
            legacyImpl = (ok && data.length == 32) ? abi.decode(data, (address)) : currentImplementation;
        }
        // The legacy assessor guest URL lives in proxy storage and is served through the
        // legacy fallback after the upgrade; capture it up front so the flow can guarantee
        // `imageInfo()` still serves it afterwards. LEGACY_ASSESSOR_GUEST_URL overrides the
        // captured value.
        string memory legacyImageUrl = vm.envOr("LEGACY_ASSESSOR_GUEST_URL", readLegacyImageUrl(marketAddress));

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
            BoundlessMarketLib.encodeConstructorArgs(BoundlessRouter(boundlessRouter), collateralToken, legacyImpl);

        if (skipSafetyChecks) {
            console2.log("WARNING: Skipping all upgrade safety checks and reference build!");
            opts.unsafeSkipAllChecks = true;
        } else {
            // Only set reference contract when doing safety checks. The file-qualified name is
            // required: the legacy fallback source (BoundlessMarketLegacy.sol) also declares a
            // contract named BoundlessMarket, so the bare name is ambiguous in any build that
            // includes contracts/src/legacy/.
            opts.referenceContract = "build-info-reference:contracts/src/BoundlessMarket.sol:BoundlessMarket";
            opts.referenceBuildInfoDir = "contracts/build-info-reference";
        }

        address newImpl = address(0);
        bytes memory initializerData = "";

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
            BoundlessMarket upgradedMarket = BoundlessMarket(payable(marketAddress));
            require(address(upgradedMarket.ROUTER()) == boundlessRouter, "upgraded market router does not match");
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
            console2.log("Upgraded BoundlessMarket router contract at %s", boundlessRouter);
            ensureLegacyImageUrl(marketAddress, legacyImageUrl);
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
        address oldImplementation = deploymentConfig.boundlessMarketOldImpl.required("boundless-market-old-impl");

        require(oldImplementation != address(0), "old implementation address is not set");
        console2.log(
            "\nWARNING: This will rollback the BoundlessMarket contract to this address: %s\n", oldImplementation
        );

        // Rollback the proxy contract.
        vm.startBroadcast(admin);

        // Previous BoundlessMarket implementations had a `setImageUrl` initializer;
        // the new shape has no upgrade-time initializer.
        bytes memory rollbackUpgradeData =
            abi.encodeWithSignature("upgradeToAndCall(address,bytes)", oldImplementation, "");

        (bool success, bytes memory returnData) = marketAddress.call(rollbackUpgradeData);
        require(success, string(returnData));

        vm.stopBroadcast();

        // Verify the upgrade
        BoundlessMarket upgradedMarket = BoundlessMarket(payable(marketAddress));
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
        // ROUTER() only exists on the router-aware impl; a rollback to the legacy impl
        // (the emergency exit of an in-place production upgrade) has none.
        (bool routerOk, bytes memory routerData) = marketAddress.staticcall(abi.encodeWithSignature("ROUTER()"));
        if (routerOk && routerData.length == 32) {
            console2.log("Upgraded BoundlessMarket router contract at %s", abi.decode(routerData, (address)));
        } else {
            console2.log("Rolled back to a legacy implementation (no ROUTER())");
        }

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
        BoundlessMarket market = BoundlessMarket(payable(marketAddress));

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
        BoundlessMarket market = BoundlessMarket(payable(marketAddress));

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
