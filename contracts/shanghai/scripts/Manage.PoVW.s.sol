// Copyright 2026 Boundless Foundation, Inc.
//
// Use of this source code is governed by the Business Source License
// as found in the LICENSE-BSL file.

pragma solidity ^0.8.26;

import {console2} from "forge-std/console2.sol";
import {Strings} from "openzeppelin/contracts/utils/Strings.sol";
import {IRiscZeroVerifier} from "risc0/IRiscZeroVerifier.sol";
import {PovwAccounting} from "../src/povw/PovwAccounting.sol";
import {PovwMint} from "../src/povw/PovwMint.sol";
import {IZKC} from "zkc/interfaces/IZKC.sol";
import {IRewards as IZKCRewards} from "zkc/interfaces/IRewards.sol";
import {ConfigLoader, DeploymentConfig} from "./Config.s.sol";
import {Upgrades} from "openzeppelin-foundry-upgrades/Upgrades.sol";
import {Options as UpgradeOptions} from "openzeppelin-foundry-upgrades/Options.sol";
import {BoundlessScriptBase, BoundlessScript} from "./BoundlessScript.s.sol";

/// @notice Upgrade script for the PovwAccounting contract.
/// @dev Set values in deployment.toml to configure the upgrade.
contract UpgradePoVWAccounting is BoundlessScriptBase {
    function run() external {
        // Load the config
        DeploymentConfig memory deploymentConfig =
            ConfigLoader.loadDeploymentConfig(string.concat(vm.projectRoot(), "/", CONFIG));

        // Get PoVW proxy address from deployment.toml
        address povwAccountingAddress = BoundlessScript.requireLib(deploymentConfig.povwAccounting, "povw-accounting");

        // Get current admin from the proxy contract
        PovwAccounting povwAccounting = PovwAccounting(povwAccountingAddress);
        address currentAdmin = povwAccounting.owner();

        address currentImplementation = Upgrades.getImplementationAddress(povwAccountingAddress);

        // Get constructor arguments for PovwAccounting
        IRiscZeroVerifier verifier =
            IRiscZeroVerifier(BoundlessScript.requireLib(deploymentConfig.verifier, "verifier"));

        // Handle ZKC address - if zero address, don't upgrade (production should have real ZKC)
        address zkcAddress = BoundlessScript.requireLib(deploymentConfig.zkc, "zkc");
        IZKC zkc = IZKC(zkcAddress);

        // Get the latest log updater ID dynamically
        bytes32 logUpdaterId;
        bool devMode = bytes(vm.envOr("RISC0_DEV_MODE", string(""))).length > 0;

        if (devMode) {
            // Use mock ID in dev mode
            logUpdaterId = bytes32(uint256(0x1111111111111111111111111111111111111111111111111111111111111111));
            console2.log("Using mock PoVW log updater ID for dev mode");
        } else {
            // Try environment variable first
            bytes32 envLogUpdater = vm.envOr("POVW_LOG_UPDATER_ID", bytes32(0));
            if (envLogUpdater != bytes32(0)) {
                logUpdaterId = envLogUpdater;
                console2.log("Using PoVW log updater ID from environment variable");
            } else {
                // Try reading from .bin file
                logUpdaterId = readImageIdFromFile("boundless-povw-log-updater.bin");
                if (logUpdaterId == bytes32(0)) {
                    // Fall back to config as last resort
                    logUpdaterId = deploymentConfig.povwLogUpdaterId;
                    console2.log("Using PoVW log updater ID from deployment config (fallback)");
                } else {
                    console2.log("Using PoVW log updater ID from .bin file");
                }
            }

            // Require that we have a valid log updater ID
            logUpdaterId = BoundlessScript.requireLib(logUpdaterId, "Log Updater ID");
        }

        console2.log("Log Updater ID: %s", vm.toString(logUpdaterId));

        UpgradeOptions memory opts;
        opts.referenceContract = "build-info-reference:PovwAccounting";
        opts.referenceBuildInfoDir = "contracts/build-info-reference";
        opts.constructorData = abi.encode(verifier, zkc, logUpdaterId);

        // Check if safety checks should be skipped
        bool skipSafetyChecks = vm.envOr("SKIP_SAFETY_CHECKS", false);
        if (skipSafetyChecks) {
            console2.log("WARNING: Skipping all upgrade safety checks (SKIP_SAFETY_CHECKS=true)");
            opts.unsafeSkipAllChecks = true;
        }

        vm.startBroadcast(currentAdmin);
        Upgrades.upgradeProxy(povwAccountingAddress, "PovwAccounting.sol:PovwAccounting", "", opts, currentAdmin);
        vm.stopBroadcast();

        // Verify the upgrade
        address newImplementation = Upgrades.getImplementationAddress(povwAccountingAddress);
        require(newImplementation != currentImplementation, "PovwAccounting implementation was not upgraded");
        require(povwAccounting.owner() == currentAdmin, "PovwAccounting admin changed during upgrade");

        console2.log("Upgraded PovwAccounting admin is %s", currentAdmin);
        console2.log("Upgraded PovwAccounting proxy contract at %s", povwAccountingAddress);
        console2.log("Upgraded PovwAccounting impl from %s to %s", currentImplementation, newImplementation);

        // Get current git commit hash
        string memory currentCommit = getCurrentCommit();

        string[] memory args = new string[](10);
        args[0] = "python3";
        args[1] = "contracts/update_deployment_toml.py";
        args[2] = "--povw-accounting-impl";
        args[3] = Strings.toHexString(newImplementation);
        args[4] = "--povw-accounting-old-impl";
        args[5] = Strings.toHexString(currentImplementation);
        args[6] = "--povw-accounting-deployment-commit";
        args[7] = currentCommit;
        args[8] = "--povw-log-updater-id";
        args[9] = vm.toString(logUpdaterId);

        vm.ffi(args);
        console2.log("Updated PovwAccounting deployment commit: %s", currentCommit);
        console2.log("Updated PoVW log updater ID: %s", vm.toString(logUpdaterId));

        // Check for uncommitted changes warning
        checkUncommittedChangesWarning("Upgrade");
    }
}

/// @notice Upgrade script for the PovwMint contract.
/// @dev Set values in deployment.toml to configure the upgrade.
contract UpgradePoVWMint is BoundlessScriptBase {
    function run() external {
        // Load the config
        DeploymentConfig memory deploymentConfig =
            ConfigLoader.loadDeploymentConfig(string.concat(vm.projectRoot(), "/", CONFIG));

        // Get PoVW proxy address from deployment.toml
        address povwMintAddress = BoundlessScript.requireLib(deploymentConfig.povwMint, "povw-mint");

        // Get current admin from the proxy contract
        PovwMint povwMint = PovwMint(povwMintAddress);
        console2.log("Getting admin");
        address currentAdmin = povwMint.owner();
        console2.log("Current PovwMint admin: %s", currentAdmin);

        console2.log("Getting impl");
        address currentImplementation = Upgrades.getImplementationAddress(povwMintAddress);

        console2.log("Current PovwMint implementation: %s", currentImplementation);

        // Get constructor arguments for PovwMint
        IRiscZeroVerifier verifier =
            IRiscZeroVerifier(BoundlessScript.requireLib(deploymentConfig.verifier, "verifier"));
        PovwAccounting povwAccounting =
            PovwAccounting(BoundlessScript.requireLib(deploymentConfig.povwAccounting, "povw-accounting"));

        bytes32 mintCalculatorId;
        bool devMode = bytes(vm.envOr("RISC0_DEV_MODE", string(""))).length > 0;

        if (devMode) {
            // Use mock ID in dev mode
            mintCalculatorId = bytes32(uint256(0x2222222222222222222222222222222222222222222222222222222222222222));
            console2.log("Using mock PoVW mint calculator ID for dev mode");
        } else {
            // Try environment variable first
            bytes32 envMintCalculator = vm.envOr("POVW_MINT_CALCULATOR_ID", bytes32(0));
            if (envMintCalculator != bytes32(0)) {
                mintCalculatorId = envMintCalculator;
                console2.log("Using PoVW mint calculator ID from environment variable");
            } else {
                // Try reading from .bin file
                mintCalculatorId = readImageIdFromFile("boundless-povw-mint-calculator.bin");
                if (mintCalculatorId == bytes32(0)) {
                    // Fall back to config as last resort
                    mintCalculatorId = deploymentConfig.povwMintCalculatorId;
                    console2.log("Using PoVW mint calculator ID from deployment config (fallback)");
                } else {
                    console2.log("Using PoVW mint calculator ID from .bin file");
                }
            }

            // Require that we have a valid mint calculator ID
            mintCalculatorId = BoundlessScript.requireLib(mintCalculatorId, "Mint Calculator ID");
        }

        console2.log("Mint Calculator ID: %s", vm.toString(mintCalculatorId));

        // Handle ZKC addresses - if zero address, don't upgrade (production should have real ZKC)
        address zkcAddress = BoundlessScript.requireLib(deploymentConfig.zkc, "zkc");
        address vezkcAddress = BoundlessScript.requireLib(deploymentConfig.vezkc, "vezkc");

        IZKC zkc = IZKC(zkcAddress);
        IZKCRewards vezkc = IZKCRewards(vezkcAddress);

        UpgradeOptions memory opts;
        opts.referenceContract = "build-info-reference:PovwMint";
        opts.referenceBuildInfoDir = "contracts/build-info-reference";
        opts.constructorData = abi.encode(verifier, povwAccounting, mintCalculatorId, zkc, vezkc);

        // Check if safety checks should be skipped
        bool skipSafetyChecks = vm.envOr("SKIP_SAFETY_CHECKS", false);
        if (skipSafetyChecks) {
            console2.log("WARNING: Skipping all upgrade safety checks (SKIP_SAFETY_CHECKS=true)");
            opts.unsafeSkipAllChecks = true;
        }

        vm.startBroadcast(currentAdmin);
        Upgrades.upgradeProxy(povwMintAddress, "PovwMint.sol:PovwMint", "", opts, currentAdmin);
        vm.stopBroadcast();

        // Verify the upgrade
        address newImplementation = Upgrades.getImplementationAddress(povwMintAddress);
        require(newImplementation != currentImplementation, "PovwMint implementation was not upgraded");
        require(povwMint.owner() == currentAdmin, "PovwMint admin changed during upgrade");

        console2.log("Upgraded PovwMint admin is %s", currentAdmin);
        console2.log("Upgraded PovwMint proxy contract at %s", povwMintAddress);
        console2.log("Upgraded PovwMint impl from %s to %s", currentImplementation, newImplementation);

        // Get current git commit hash
        string memory currentCommit = getCurrentCommit();

        string[] memory args = new string[](10);
        args[0] = "python3";
        args[1] = "contracts/update_deployment_toml.py";
        args[2] = "--povw-mint-impl";
        args[3] = Strings.toHexString(newImplementation);
        args[4] = "--povw-mint-old-impl";
        args[5] = Strings.toHexString(currentImplementation);
        args[6] = "--povw-mint-deployment-commit";
        args[7] = currentCommit;
        args[8] = "--povw-mint-calculator-id";
        args[9] = vm.toString(mintCalculatorId);

        vm.ffi(args);
        console2.log("Updated PovwMint deployment commit: %s", currentCommit);
        console2.log("Updated PoVW mint calculator ID: %s", vm.toString(mintCalculatorId));

        // Check for uncommitted changes warning
        checkUncommittedChangesWarning("Upgrade");
    }
}

/// @notice Script for transferring ownership of the PoVW contracts.
/// @dev Transfer will be from the current owner to the NEW_ADMIN environment variable
contract TransferPoVWOwnership is BoundlessScriptBase {
    function run() external {
        // Load the config
        DeploymentConfig memory deploymentConfig =
            ConfigLoader.loadDeploymentConfig(string.concat(vm.projectRoot(), "/", CONFIG));

        address newAdmin = BoundlessScript.requireLib(vm.envOr("NEW_ADMIN", address(0)), "NEW_ADMIN");
        address povwAccountingAddress = BoundlessScript.requireLib(deploymentConfig.povwAccounting, "povw-accounting");
        address povwMintAddress = BoundlessScript.requireLib(deploymentConfig.povwMint, "povw-mint");

        PovwAccounting povwAccounting = PovwAccounting(povwAccountingAddress);
        PovwMint povwMint = PovwMint(povwMintAddress);

        address currentAccountingAdmin = povwAccounting.owner();
        address currentMintAdmin = povwMint.owner();

        require(newAdmin != currentAccountingAdmin, "current and new PovwAccounting admin address are the same");
        require(newAdmin != currentMintAdmin, "current and new PovwMint admin address are the same");

        vm.startBroadcast(currentAccountingAdmin);
        povwAccounting.transferOwnership(newAdmin);
        vm.stopBroadcast();

        vm.startBroadcast(currentMintAdmin);
        povwMint.transferOwnership(newAdmin);
        vm.stopBroadcast();

        // check owners of each contract
        require(povwAccounting.owner() == newAdmin, "PovwAccounting owner is not the new admin");
        require(povwMint.owner() == newAdmin, "PovwMint owner is not the new admin");

        console2.log("Transferred ownership of PovwAccounting contract from %s to %s", currentAccountingAdmin, newAdmin);
        console2.log("Transferred ownership of PovwMint contract from %s to %s", currentMintAdmin, newAdmin);

        // Update deployment.toml with new admin addresses
        string[] memory args = new string[](6);
        args[0] = "python3";
        args[1] = "contracts/update_deployment_toml.py";
        args[2] = "--povw-accounting-admin";
        args[3] = Strings.toHexString(newAdmin);
        args[4] = "--povw-mint-admin";
        args[5] = Strings.toHexString(newAdmin);
        vm.ffi(args);

        console2.log("Updated deployment.toml with new admin addresses");
    }
}

/// @notice Rollback script for the PovwAccounting contract.
/// @dev Set values in deployment.toml to configure the rollback.
contract RollbackPoVWAccounting is BoundlessScriptBase {
    function run() external {
        // Load the config
        DeploymentConfig memory deploymentConfig =
            ConfigLoader.loadDeploymentConfig(string.concat(vm.projectRoot(), "/", CONFIG));

        address povwAccountingAddress = BoundlessScript.requireLib(deploymentConfig.povwAccounting, "povw-accounting");
        address oldImplementation =
            BoundlessScript.requireLib(deploymentConfig.povwAccountingOldImpl, "povw-accounting-old-impl");

        // Get current admin from the proxy contract
        PovwAccounting povwAccounting = PovwAccounting(povwAccountingAddress);
        address currentAdmin = povwAccounting.owner();

        require(oldImplementation != address(0), "old implementation address is not set");
        console2.log(
            "\nWARNING: This will rollback the PovwAccounting contract to this address: %s\n", oldImplementation
        );

        // Rollback the proxy contract
        vm.startBroadcast(currentAdmin);

        // For PovwAccounting, we don't need a reinitializer call like BoundlessMarket
        bytes memory rollbackUpgradeData = abi.encodeWithSignature("upgradeTo(address)", oldImplementation);
        (bool success, bytes memory returnData) = povwAccountingAddress.call(rollbackUpgradeData);
        require(success, string(returnData));

        vm.stopBroadcast();

        // Verify the rollback
        address currentImplementation = Upgrades.getImplementationAddress(povwAccountingAddress);
        require(currentImplementation == oldImplementation, "PovwAccounting rollback failed");
        require(povwAccounting.owner() == currentAdmin, "PovwAccounting admin changed during rollback");
        console2.log("Rollback successful. PovwAccounting implementation is now %s", currentImplementation);

        // Update deployment.toml to swap impl and old-impl addresses
        string[] memory args = new string[](6);
        args[0] = "python3";
        args[1] = "contracts/update_deployment_toml.py";
        args[2] = "--povw-accounting-impl";
        args[3] = Strings.toHexString(currentImplementation);
        args[4] = "--povw-accounting-old-impl";
        args[5] = Strings.toHexString(deploymentConfig.povwAccountingImpl);

        vm.ffi(args);
        console2.log("Updated deployment.toml with rollback addresses");
    }
}

/// @notice Rollback script for the PovwMint contract.
/// @dev Set values in deployment.toml to configure the rollback.
contract RollbackPoVWMint is BoundlessScriptBase {
    function run() external {
        // Load the config
        DeploymentConfig memory deploymentConfig =
            ConfigLoader.loadDeploymentConfig(string.concat(vm.projectRoot(), "/", CONFIG));

        address povwMintAddress = BoundlessScript.requireLib(deploymentConfig.povwMint, "povw-mint");
        address oldImplementation = BoundlessScript.requireLib(deploymentConfig.povwMintOldImpl, "povw-mint-old-impl");

        // Get current admin from the proxy contract
        PovwMint povwMint = PovwMint(povwMintAddress);
        address currentAdmin = povwMint.owner();

        require(oldImplementation != address(0), "old implementation address is not set");
        console2.log("\nWARNING: This will rollback the PovwMint contract to this address: %s\n", oldImplementation);

        // Rollback the proxy contract
        vm.startBroadcast(currentAdmin);

        // For PovwMint, we don't need a reinitializer call like BoundlessMarket
        bytes memory rollbackUpgradeData = abi.encodeWithSignature("upgradeTo(address)", oldImplementation);
        (bool success, bytes memory returnData) = povwMintAddress.call(rollbackUpgradeData);
        require(success, string(returnData));

        vm.stopBroadcast();

        // Verify the rollback
        address currentImplementation = Upgrades.getImplementationAddress(povwMintAddress);
        require(currentImplementation == oldImplementation, "PovwMint rollback failed");
        require(povwMint.owner() == currentAdmin, "PovwMint admin changed during rollback");
        console2.log("Rollback successful. PovwMint implementation is now %s", currentImplementation);

        // Update deployment.toml to swap impl and old-impl addresses
        string[] memory args = new string[](6);
        args[0] = "python3";
        args[1] = "contracts/update_deployment_toml.py";
        args[2] = "--povw-mint-impl";
        args[3] = Strings.toHexString(currentImplementation);
        args[4] = "--povw-mint-old-impl";
        args[5] = Strings.toHexString(deploymentConfig.povwMintImpl);

        vm.ffi(args);
        console2.log("Updated deployment.toml with rollback addresses");
    }
}
