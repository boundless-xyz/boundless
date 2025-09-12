// Copyright 2025 RISC Zero, Inc.
//
// Use of this source code is governed by the Business Source License
// as found in the LICENSE-BSL file.

pragma solidity ^0.8.9;

import {Script} from "forge-std/Script.sol";
import {console2} from "forge-std/console2.sol";
import {Strings} from "openzeppelin/contracts/utils/Strings.sol";
import {IRiscZeroVerifier} from "risc0/IRiscZeroVerifier.sol";
import {PovwAccounting} from "../src/povw/PovwAccounting.sol";
import {PovwMint} from "../src/povw/PovwMint.sol";
import {IZKC} from "zkc/interfaces/IZKC.sol";
import {IRewards as IZKCRewards} from "zkc/interfaces/IRewards.sol";
import {ConfigLoader, DeploymentConfig, ConfigParser} from "./Config.s.sol";
import {ERC1967Proxy} from "@openzeppelin/contracts/proxy/ERC1967/ERC1967Proxy.sol";
import {Upgrades} from "openzeppelin-foundry-upgrades/Upgrades.sol";
import {Options as UpgradeOptions} from "openzeppelin-foundry-upgrades/Options.sol";
import {PoVWScript, PoVWLib} from "./PoVWLib.s.sol";

/// @notice Upgrade script for the PovwAccounting contract.
/// @dev Set values in deployment.toml to configure the upgrade.
contract UpgradePoVWAccounting is PoVWScript {
    function run() external {
        // Load the config
        DeploymentConfig memory deploymentConfig =
            ConfigLoader.loadDeploymentConfig(string.concat(vm.projectRoot(), "/", CONFIG));

        // Get PoVW proxy address from deployment.toml
        address povwAccountingAddress = PoVWLib.requireLib(deploymentConfig.povwAccounting, "povw-accounting");

        // Get current admin from the proxy contract
        PovwAccounting povwAccounting = PovwAccounting(povwAccountingAddress);
        address currentAdmin = povwAccounting.owner();

        address currentImplementation = Upgrades.getImplementationAddress(povwAccountingAddress);

        // Get constructor arguments for PovwAccounting
        IRiscZeroVerifier verifier = IRiscZeroVerifier(PoVWLib.requireLib(deploymentConfig.verifier, "verifier"));

        // Handle ZKC address - if zero address, don't upgrade (production should have real ZKC)
        address zkcAddress = PoVWLib.requireLib(deploymentConfig.zkc, "zkc");
        IZKC zkc = IZKC(zkcAddress);

        bytes32 logUpdaterId = PoVWLib.requireLib(deploymentConfig.povwLogUpdaterId, "povw-log-updater-id");

        UpgradeOptions memory opts;
        opts.referenceContract = "build-info-reference:PovwAccounting";
        opts.referenceBuildInfoDir = "contracts/build-info-reference";
        opts.constructorData = abi.encode(verifier, zkc, logUpdaterId);

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

        string[] memory args = new string[](8);
        args[0] = "python3";
        args[1] = "contracts/update_deployment_toml.py";
        args[2] = "--povw-accounting-impl";
        args[3] = Strings.toHexString(newImplementation);
        args[4] = "--povw-accounting-old-impl";
        args[5] = Strings.toHexString(currentImplementation);
        args[6] = "--povw-accounting-deployment-commit";
        args[7] = currentCommit;

        vm.ffi(args);
        console2.log("Updated PovwAccounting deployment commit: %s", currentCommit);

        // Check for uncommitted changes warning
        checkUncommittedChangesWarning("Upgrade");
    }
}

/// @notice Upgrade script for the PovwMint contract.
/// @dev Set values in deployment.toml to configure the upgrade.
contract UpgradePoVWMint is PoVWScript {
    function run() external {
        // Load the config
        DeploymentConfig memory deploymentConfig =
            ConfigLoader.loadDeploymentConfig(string.concat(vm.projectRoot(), "/", CONFIG));

        // Get PoVW proxy address from deployment.toml
        address povwMintAddress = PoVWLib.requireLib(deploymentConfig.povwMint, "povw-mint");

        // Get current admin from the proxy contract
        PovwMint povwMint = PovwMint(povwMintAddress);
        console2.log("Getting admin");
        address currentAdmin = povwMint.owner();
        console2.log("Current PovwMint admin: %s", currentAdmin);

        console2.log("Getting impl");
        address currentImplementation = Upgrades.getImplementationAddress(povwMintAddress);

        console2.log("Current PovwMint implementation: %s", currentImplementation);

        // Get constructor arguments for PovwMint
        IRiscZeroVerifier verifier = IRiscZeroVerifier(PoVWLib.requireLib(deploymentConfig.verifier, "verifier"));
        PovwAccounting povwAccounting =
            PovwAccounting(PoVWLib.requireLib(deploymentConfig.povwAccounting, "povw-accounting"));
        bytes32 mintCalculatorId = PoVWLib.requireLib(deploymentConfig.povwMintCalculatorId, "povw-mint-calculator-id");

        // Handle ZKC addresses - if zero address, don't upgrade (production should have real ZKC)
        address zkcAddress = PoVWLib.requireLib(deploymentConfig.zkc, "zkc");
        address vezkcAddress = PoVWLib.requireLib(deploymentConfig.vezkc, "vezkc");

        IZKC zkc = IZKC(zkcAddress);
        IZKCRewards vezkc = IZKCRewards(vezkcAddress);

        UpgradeOptions memory opts;
        opts.referenceContract = "build-info-reference:PovwMint";
        opts.referenceBuildInfoDir = "contracts/build-info-reference";
        opts.constructorData = abi.encode(verifier, povwAccounting, mintCalculatorId, zkc, vezkc);

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

        string[] memory args = new string[](8);
        args[0] = "python3";
        args[1] = "contracts/update_deployment_toml.py";
        args[2] = "--povw-mint-impl";
        args[3] = Strings.toHexString(newImplementation);
        args[4] = "--povw-mint-old-impl";
        args[5] = Strings.toHexString(currentImplementation);
        args[6] = "--povw-mint-deployment-commit";
        args[7] = currentCommit;

        vm.ffi(args);
        console2.log("Updated PovwMint deployment commit: %s", currentCommit);

        // Check for uncommitted changes warning
        checkUncommittedChangesWarning("Upgrade");
    }
}

/// @notice Script for transferring ownership of the PoVW contracts.
/// @dev Transfer will be from the current owner to the NEW_ADMIN environment variable
contract TransferPoVWOwnership is PoVWScript {
    function run() external {
        // Load the config
        DeploymentConfig memory deploymentConfig =
            ConfigLoader.loadDeploymentConfig(string.concat(vm.projectRoot(), "/", CONFIG));

        address newAdmin = PoVWLib.requireLib(vm.envOr("NEW_ADMIN", address(0)), "NEW_ADMIN");
        address povwAccountingAddress = PoVWLib.requireLib(deploymentConfig.povwAccounting, "povw-accounting");
        address povwMintAddress = PoVWLib.requireLib(deploymentConfig.povwMint, "povw-mint");

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

        console2.log("Transferred ownership of PovwAccounting contract from %s to %s", currentAccountingAdmin, newAdmin);
        console2.log("Transferred ownership of PovwMint contract from %s to %s", currentMintAdmin, newAdmin);
        console2.log("Ownership transfer is immediate with regular Ownable (no acceptance required)");
    }
}

/// @notice Rollback script for the PovwAccounting contract.
/// @dev Set values in deployment.toml to configure the rollback.
contract RollbackPoVWAccounting is PoVWScript {
    function run() external {
        // Load the config
        DeploymentConfig memory deploymentConfig =
            ConfigLoader.loadDeploymentConfig(string.concat(vm.projectRoot(), "/", CONFIG));

        address povwAccountingAddress = PoVWLib.requireLib(deploymentConfig.povwAccounting, "povw-accounting");
        address oldImplementation =
            PoVWLib.requireLib(deploymentConfig.povwAccountingOldImpl, "povw-accounting-old-impl");

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
contract RollbackPoVWMint is PoVWScript {
    function run() external {
        // Load the config
        DeploymentConfig memory deploymentConfig =
            ConfigLoader.loadDeploymentConfig(string.concat(vm.projectRoot(), "/", CONFIG));

        address povwMintAddress = PoVWLib.requireLib(deploymentConfig.povwMint, "povw-mint");
        address oldImplementation = PoVWLib.requireLib(deploymentConfig.povwMintOldImpl, "povw-mint-old-impl");

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

/// @notice Script for transferring ownership of the PovwMint contract.
/// @dev Transfer will be from the current owner to the NEW_ADMIN environment variable
contract TransferPoVWMintOwnership is PoVWScript {
    function run() external {
        // Load the config
        DeploymentConfig memory deploymentConfig =
            ConfigLoader.loadDeploymentConfig(string.concat(vm.projectRoot(), "/", CONFIG));

        address newAdmin = PoVWLib.requireLib(vm.envOr("NEW_ADMIN", address(0)), "NEW_ADMIN");
        address povwMintAddress = PoVWLib.requireLib(deploymentConfig.povwMint, "povw-mint");
        PovwMint povwMint = PovwMint(povwMintAddress);

        address currentAdmin = povwMint.owner();
        require(newAdmin != currentAdmin, "current and new admin address are the same");

        vm.broadcast(currentAdmin);
        povwMint.transferOwnership(newAdmin);

        console2.log("Transferred ownership of the PovwMint contract from %s to %s", currentAdmin, newAdmin);
        console2.log("Ownership transfer is immediate with regular Ownable (no acceptance required)");
    }
}
