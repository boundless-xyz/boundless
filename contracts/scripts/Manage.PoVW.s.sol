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
import {IZKC, IZKCRewards} from "../src/povw/IZKC.sol";
import {ConfigLoader, DeploymentConfig, ConfigParser} from "./Config.s.sol";
import {ERC1967Proxy} from "@openzeppelin/contracts/proxy/ERC1967/ERC1967Proxy.sol";
import {Upgrades} from "openzeppelin-foundry-upgrades/Upgrades.sol";
import {Options as UpgradeOptions} from "openzeppelin-foundry-upgrades/Options.sol";

library RequireLibPoVW {
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

using RequireLibPoVW for address;
using RequireLibPoVW for string;
using RequireLibPoVW for bytes32;

// This is the EIP-1967 implementation slot:
bytes32 constant IMPLEMENTATION_SLOT = 0x360894A13BA1A3210667C828492DB98DCA3E2076CC3735A920A3CA505D382BBC;

/// @notice Base contract for the PoVW scripts below, providing common context and functions.
contract PoVWScript is Script {
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


/// @notice Upgrade script for the PovwAccounting contract.
/// @dev Set values in deployment.toml to configure the upgrade.
contract UpgradePoVWAccounting is PoVWScript {
    function run() external {
        // Load the config
        DeploymentConfig memory deploymentConfig =
            ConfigLoader.loadDeploymentConfig(string.concat(vm.projectRoot(), "/", CONFIG));

        address admin = deploymentConfig.admin.required("admin");
        address verifier = deploymentConfig.verifier.required("verifier");
        
        // Get PoVW addresses from environment or deployment.toml
        address povwAccountingAddress = vm.envOr("POVW_ACCOUNTING_ADDRESS", address(0)).required("povw-accounting-address");
        bytes32 logUpdaterId = vm.envOr("POVW_LOG_UPDATER_ID", bytes32(0)).required("povw-log-updater-id");
        address zkcToken = vm.envOr("ZKC_TOKEN_ADDRESS", address(0)).required("zkc-token-address");
        
        address currentImplementation = address(uint160(uint256(vm.load(povwAccountingAddress, IMPLEMENTATION_SLOT))));

        UpgradeOptions memory opts;
        // Note: Constructor args encoding would need to be implemented similar to BoundlessMarketLib
        opts.referenceContract = "build-info-reference:PovwAccounting";
        opts.referenceBuildInfoDir = "contracts/build-info-reference";

        vm.startBroadcast(admin);
        Upgrades.upgradeProxy(povwAccountingAddress, "PovwAccounting.sol:PovwAccounting", "", opts, admin);
        vm.stopBroadcast();

        // Verify the upgrade
        PovwAccounting upgradedContract = PovwAccounting(povwAccountingAddress);
        require(upgradedContract.VERIFIER() == IRiscZeroVerifier(verifier), "upgraded PovwAccounting verifier does not match");
        require(upgradedContract.LOG_UPDATER_ID() == logUpdaterId, "upgraded PovwAccounting log updater ID does not match");
        require(upgradedContract.owner() == admin, "upgraded PovwAccounting admin does not match");

        address newImplementation = address(uint160(uint256(vm.load(povwAccountingAddress, IMPLEMENTATION_SLOT))));

        console2.log("Upgraded PovwAccounting admin is %s", admin);
        console2.log("Upgraded PovwAccounting proxy contract at %s", povwAccountingAddress);
        console2.log("Upgraded PovwAccounting impl contract at %s", newImplementation);

        // Get current git commit hash
        string[] memory gitArgs = new string[](2);
        gitArgs[0] = "git";
        gitArgs[1] = "rev-parse HEAD";
        string memory currentCommit = string(vm.ffi(gitArgs));

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
    }
}

/// @notice Upgrade script for the PovwMint contract.
/// @dev Set values in deployment.toml to configure the upgrade.
contract UpgradePoVWMint is PoVWScript {
    function run() external {
        // Load the config
        DeploymentConfig memory deploymentConfig =
            ConfigLoader.loadDeploymentConfig(string.concat(vm.projectRoot(), "/", CONFIG));

        address admin = deploymentConfig.admin.required("admin");
        address verifier = deploymentConfig.verifier.required("verifier");
        
        // Get PoVW addresses from environment or deployment.toml
        address povwMintAddress = vm.envOr("POVW_MINT_ADDRESS", address(0)).required("povw-mint-address");
        address povwAccountingAddress = vm.envOr("POVW_ACCOUNTING_ADDRESS", address(0)).required("povw-accounting-address");
        bytes32 mintCalculatorId = vm.envOr("POVW_MINT_CALCULATOR_ID", bytes32(0)).required("povw-mint-calculator-id");
        address zkcToken = vm.envOr("ZKC_TOKEN_ADDRESS", address(0)).required("zkc-token-address");
        address zkcRewards = vm.envOr("ZKC_REWARDS_ADDRESS", address(0)).required("zkc-rewards-address");
        
        address currentImplementation = address(uint160(uint256(vm.load(povwMintAddress, IMPLEMENTATION_SLOT))));

        UpgradeOptions memory opts;
        opts.referenceContract = "build-info-reference:PovwMint";
        opts.referenceBuildInfoDir = "contracts/build-info-reference";

        vm.startBroadcast(admin);
        Upgrades.upgradeProxy(povwMintAddress, "PovwMint.sol:PovwMint", "", opts, admin);
        vm.stopBroadcast();

        // Verify the upgrade
        PovwMint upgradedContract = PovwMint(povwMintAddress);
        require(upgradedContract.owner() == admin, "upgraded PovwMint admin does not match");

        address newImplementation = address(uint160(uint256(vm.load(povwMintAddress, IMPLEMENTATION_SLOT))));

        console2.log("Upgraded PovwMint admin is %s", admin);
        console2.log("Upgraded PovwMint proxy contract at %s", povwMintAddress);
        console2.log("Upgraded PovwMint impl contract at %s", newImplementation);

        // Get current git commit hash
        string[] memory gitArgs = new string[](2);
        gitArgs[0] = "git";
        gitArgs[1] = "rev-parse HEAD";
        string memory currentCommit = string(vm.ffi(gitArgs));

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
    }
}

/// @notice Script for transferring ownership of the PoVW contracts.
/// @dev Transfer will be from the current admin (i.e. owner) address to the admin address set in deployment.toml
contract TransferPoVWOwnership is PoVWScript {
    function run() external {
        // Load the config
        DeploymentConfig memory deploymentConfig =
            ConfigLoader.loadDeploymentConfig(string.concat(vm.projectRoot(), "/", CONFIG));

        address admin = deploymentConfig.admin.required("admin");
        address povwAccountingAddress = vm.envOr("POVW_ACCOUNTING_ADDRESS", address(0)).required("povw-accounting-address");
        address povwMintAddress = vm.envOr("POVW_MINT_ADDRESS", address(0)).required("povw-mint-address");
        
        PovwAccounting povwAccounting = PovwAccounting(povwAccountingAddress);
        PovwMint povwMint = PovwMint(povwMintAddress);

        address currentAccountingAdmin = povwAccounting.owner();
        address currentMintAdmin = povwMint.owner();
        
        require(admin != currentAccountingAdmin, "current and new PovwAccounting admin address are the same");
        require(admin != currentMintAdmin, "current and new PovwMint admin address are the same");

        vm.startBroadcast(currentAccountingAdmin);
        povwAccounting.transferOwnership(admin);
        vm.stopBroadcast();

        vm.startBroadcast(currentMintAdmin);
        povwMint.transferOwnership(admin);
        vm.stopBroadcast();

        console2.log("Transferred ownership of PovwAccounting contract from %s to %s", currentAccountingAdmin, admin);
        console2.log("Transferred ownership of PovwMint contract from %s to %s", currentMintAdmin, admin);
        console2.log("Ownership must be accepted by the new admin %s", admin);
    }
}


/// @notice Rollback script for the PovwAccounting contract.
/// @dev Set values in deployment.toml to configure the rollback.
contract RollbackPoVWAccounting is PoVWScript {
    function run() external {
        // Load the config
        DeploymentConfig memory deploymentConfig =
            ConfigLoader.loadDeploymentConfig(string.concat(vm.projectRoot(), "/", CONFIG));

        address admin = deploymentConfig.admin.required("admin");
        address povwAccountingAddress = deploymentConfig.povwAccounting.required("povw-accounting");
        address oldImplementation = deploymentConfig.povwAccountingOldImpl.required("povw-accounting-old-impl");

        require(oldImplementation != address(0), "old implementation address is not set");
        console2.log(
            "\nWARNING: This will rollback the PovwAccounting contract to this address: %s\n", oldImplementation
        );

        // Rollback the proxy contract
        vm.startBroadcast(admin);

        // For PovwAccounting, we don't need a reinitializer call like BoundlessMarket
        bytes memory rollbackUpgradeData = abi.encodeWithSignature("upgradeTo(address)", oldImplementation);
        (bool success, bytes memory returnData) = povwAccountingAddress.call(rollbackUpgradeData);
        require(success, string(returnData));

        vm.stopBroadcast();

        // Verify the rollback
        PovwAccounting rolledBackContract = PovwAccounting(povwAccountingAddress);
        require(rolledBackContract.owner() == admin, "rolled back PovwAccounting admin does not match");
        require(
            address(rolledBackContract.VERIFIER()) == deploymentConfig.verifier,
            "rolled back PovwAccounting verifier does not match"
        );

        address currentImplementation = address(uint160(uint256(vm.load(povwAccountingAddress, IMPLEMENTATION_SLOT))));
        require(
            currentImplementation == oldImplementation,
            "current implementation address does not match the old implementation address"
        );
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
    }
}

/// @notice Rollback script for the PovwMint contract.
/// @dev Set values in deployment.toml to configure the rollback.
contract RollbackPoVWMint is PoVWScript {
    function run() external {
        // Load the config
        DeploymentConfig memory deploymentConfig =
            ConfigLoader.loadDeploymentConfig(string.concat(vm.projectRoot(), "/", CONFIG));

        address admin = deploymentConfig.admin.required("admin");
        address povwMintAddress = deploymentConfig.povwMint.required("povw-mint");
        address oldImplementation = deploymentConfig.povwMintOldImpl.required("povw-mint-old-impl");

        require(oldImplementation != address(0), "old implementation address is not set");
        console2.log(
            "\nWARNING: This will rollback the PovwMint contract to this address: %s\n", oldImplementation
        );

        // Rollback the proxy contract
        vm.startBroadcast(admin);

        // For PovwMint, we don't need a reinitializer call like BoundlessMarket
        bytes memory rollbackUpgradeData = abi.encodeWithSignature("upgradeTo(address)", oldImplementation);
        (bool success, bytes memory returnData) = povwMintAddress.call(rollbackUpgradeData);
        require(success, string(returnData));

        vm.stopBroadcast();

        // Verify the rollback
        PovwMint rolledBackContract = PovwMint(povwMintAddress);
        require(rolledBackContract.owner() == admin, "rolled back PovwMint admin does not match");

        address currentImplementation = address(uint160(uint256(vm.load(povwMintAddress, IMPLEMENTATION_SLOT))));
        require(
            currentImplementation == oldImplementation,
            "current implementation address does not match the old implementation address"
        );
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
    }
}

/// @notice Script for transferring ownership of the PovwMint contract.
/// @dev Transfer will be from the current admin (i.e. owner) address to the admin address set in deployment.toml
///
/// See the Foundry documentation for more information about Solidity scripts.
/// https://book.getfoundry.sh/tutorials/solidity-scripting
contract TransferPoVWMintOwnership is PoVWScript {
    function run() external {
        // Load the config
        DeploymentConfig memory deploymentConfig =
            ConfigLoader.loadDeploymentConfig(string.concat(vm.projectRoot(), "/", CONFIG));

        address admin = deploymentConfig.povwMintAdmin.required("povw-mint-admin");
        address povwMintAddress = deploymentConfig.povwMint.required("povw-mint");
        PovwMint povwMint = PovwMint(povwMintAddress);

        address currentAdmin = povwMint.owner();
        require(admin != currentAdmin, "current and new admin address are the same");

        vm.broadcast(currentAdmin);
        povwMint.transferOwnership(admin);

        console2.log("Transferred ownership of the PovwMint contract from %s to %s", currentAdmin, admin);
        console2.log("Ownership transfer is immediate with regular Ownable (no acceptance required)");
    }
}