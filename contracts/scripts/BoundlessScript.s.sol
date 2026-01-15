// Copyright 2026 Boundless Foundation, Inc.
//
// Use of this source code is governed by the Business Source License
// as found in the LICENSE-BSL file.
// SPDX-License-Identifier: BUSL-1.1

pragma solidity ^0.8.26;

import {Script, console2} from "forge-std/Script.sol";
import {Strings} from "@openzeppelin/contracts/utils/Strings.sol";
import {ConfigLoader, DeploymentConfig} from "./Config.s.sol";

library BoundlessScript {
    /// @notice Validates that an address value is not zero, with descriptive error message
    function requireLib(address value, string memory label) internal pure returns (address) {
        if (value == address(0)) {
            console2.log("address value %s is required", label);
            require(false, "required address value not set");
        }
        return value;
    }

    /// @notice Validates that a bytes32 value is not zero, with descriptive error message
    function requireLib(bytes32 value, string memory label) internal pure returns (bytes32) {
        if (value == bytes32(0)) {
            console2.log("bytes32 value %s is required", label);
            require(false, "required bytes32 value not set");
        }
        return value;
    }

    /// @notice Validates that a string value is not empty, with descriptive error message
    function requireLib(string memory value, string memory label) internal pure returns (string memory) {
        if (bytes(value).length == 0) {
            console2.log("string value %s is required", label);
            require(false, "required string value not set");
        }
        return value;
    }

    /// @notice Helper to convert string to lowercase for display
    function _toLowerCase(string memory str) internal pure returns (string memory) {
        bytes memory strBytes = bytes(str);
        for (uint256 i = 0; i < strBytes.length; i++) {
            if (strBytes[i] >= 0x41 && strBytes[i] <= 0x5A) {
                strBytes[i] = bytes1(uint8(strBytes[i]) + 32);
            }
        }
        return string(strBytes);
    }
}

/// @notice Base contract for Boundless scripts with shared functionality
abstract contract BoundlessScriptBase is Script {
    using BoundlessScript for address;
    using BoundlessScript for bytes32;
    using BoundlessScript for string;

    // Path to deployment config file, relative to the project root.
    string constant CONFIG = "contracts/deployment.toml";

    /// @notice Gets the current git commit hash
    function getCurrentCommit() internal view returns (string memory) {
        return vm.envOr("CURRENT_COMMIT", string("unknown"));
    }

    /// @notice Displays warning for uncommitted changes
    function checkUncommittedChangesWarning(string memory actionType) internal view {
        string memory hasUnstaged = vm.envOr("HAS_UNSTAGED_CHANGES", string(""));
        string memory hasStaged = vm.envOr("HAS_STAGED_CHANGES", string(""));
        if (bytes(hasUnstaged).length > 0 || bytes(hasStaged).length > 0) {
            console2.log("");
            console2.log("=================================================================");
            console2.log(string.concat("WARNING: ", actionType, " was done with uncommitted changes!"));
            console2.log(string.concat("- The ", actionType, " commit hash may not reflect actual code state"));
            console2.log(
                string.concat(
                    "- Consider committing changes before production ", BoundlessScript._toLowerCase(actionType), "s"
                )
            );
            console2.log("=================================================================");
        }
    }

    /// @notice Gets the deployer address from private key or env var
    function getDeployer() internal returns (address) {
        uint256 privateKey = vm.envOr("DEPLOYER_PRIVATE_KEY", uint256(0));
        if (privateKey != 0) {
            vm.rememberKey(privateKey);
            return vm.addr(privateKey);
        }

        address deployer = vm.envOr("DEPLOYER_ADDRESS", address(0));
        require(deployer != address(0), "env var DEPLOYER_ADDRESS or DEPLOYER_PRIVATE_KEY required");
        return deployer;
    }

    /// @notice Reads a 32-byte image ID from a .bin file using r0vm --id
    function readImageIdFromFile(string memory filename) internal returns (bytes32) {
        string memory filePath = string.concat(vm.projectRoot(), "/crates/povw/elfs/", filename);

        string[] memory args = new string[](4);
        args[0] = "r0vm";
        args[1] = "--id";
        args[2] = "--elf";
        args[3] = filePath;

        try vm.ffi(args) returns (bytes memory result) {
            return abi.decode(result, (bytes32));
        } catch {
            console2.log("Failed to read image ID from .bin file: %s", filename);
            return bytes32(0);
        }
    }

    /// @notice Updates a specific field in deployment.toml via FFI
    /// @param key The field name to update (e.g., "admin", "admin-2")
    /// @param value The address value to set
    function _updateDeploymentConfig(string memory key, address value) internal {
        string[] memory args = new string[](4);
        args[0] = "python3";
        args[1] = "contracts/update_deployment_toml.py";
        args[2] = string.concat("--", key);
        args[3] = Strings.toHexString(value);

        vm.ffi(args);
    }

    /// @notice Removes an admin from deployment.toml by clearing the matching admin field
    /// @param adminField1 First admin field to check (e.g., "admin")
    /// @param adminField2 Second admin field to check (e.g., "admin-2")
    /// @param removedAdmin The admin address being removed
    /// @dev Only clears the TOML field that contains the specific admin address being removed
    function _removeAdminFromToml(string memory adminField1, string memory adminField2, address removedAdmin) internal {
        // Load current deployment config to check which field contains the removed admin
        DeploymentConfig memory deploymentConfig =
            ConfigLoader.loadDeploymentConfig(string.concat(vm.projectRoot(), "/", CONFIG));

        // Clear the field that matches the removed admin address
        if (deploymentConfig.admin == removedAdmin) {
            _updateDeploymentConfig(adminField1, address(0));
            console2.log("Cleared %s field in deployment.toml", adminField1);
        } else if (deploymentConfig.admin2 == removedAdmin) {
            _updateDeploymentConfig(adminField2, address(0));
            console2.log("Cleared %s field in deployment.toml", adminField2);
        } else {
            console2.log(
                "Admin address %s not found in TOML fields, no update needed", Strings.toHexString(removedAdmin)
            );
        }
    }

    /// @notice Print Gnosis Safe transaction information for manual upgrades
    /// @param proxyAddress The proxy contract address (target for Gnosis Safe)
    /// @param newImpl The new implementation address
    /// @param initializerData The initializer call data (if any)
    function _printGnosisSafeInfo(address proxyAddress, address newImpl, bytes memory initializerData) internal pure {
        console2.log("================================");
        console2.log("================================");
        console2.log("=== GNOSIS SAFE UPGRADE INFO ===");
        console2.log("Target Address (To): ", proxyAddress);

        if (initializerData.length > 0) {
            // For upgradeToAndCall
            bytes memory callData = abi.encodeWithSignature("upgradeToAndCall(address,bytes)", newImpl, initializerData);
            console2.log("Function: upgradeToAndCall(address,bytes)");
            console2.log("New Implementation: ", newImpl);
            console2.log("Calldata:");
            console2.logBytes(callData);
            console2.log("");
            console2.log("Expected Events on Successful Execution:");
            console2.log("1. Upgraded(address indexed implementation)");
            console2.log("   - implementation: ", newImpl);
        } else {
            // For upgradeTo
            bytes memory callData = abi.encodeWithSignature("upgradeTo(address)", newImpl);
            console2.log("Function: upgradeTo(address)");
            console2.log("New Implementation: ", newImpl);
            console2.log("Calldata:");
            console2.logBytes(callData);
            console2.log("");
            console2.log("Expected Events on Successful Execution:");
            console2.log("1. Upgraded(address indexed implementation)");
            console2.log("   - implementation: ", newImpl);
        }
        console2.log("================================");
    }
}
