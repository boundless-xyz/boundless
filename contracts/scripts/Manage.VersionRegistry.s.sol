// Copyright 2026 Boundless Foundation, Inc.
//
// Use of this source code is governed by the Business Source License
// as found in the LICENSE-BSL file.
// SPDX-License-Identifier: BUSL-1.1

pragma solidity ^0.8.26;

import {Vm} from "forge-std/Vm.sol";
import {console2} from "forge-std/console2.sol";
import {stdToml} from "forge-std/StdToml.sol";
import {Strings} from "openzeppelin/contracts/utils/Strings.sol";
import {VersionRegistry} from "../src/VersionRegistry.sol";
import {ERC1967Proxy} from "@openzeppelin/contracts/proxy/ERC1967/ERC1967Proxy.sol";
import {Upgrades} from "openzeppelin-foundry-upgrades/Upgrades.sol";
import {Options as UpgradeOptions} from "openzeppelin-foundry-upgrades/Options.sol";
import {BoundlessScriptBase, BoundlessScript} from "./BoundlessScript.s.sol";

/// @notice Config struct for the VersionRegistry deployment TOML.
struct VersionRegistryConfig {
    string name;
    uint256 chainId;
    address admin;
    address versionRegistry;
    address versionRegistryImpl;
    string minimumBrokerVersion;
    string notice;
}

/// @notice Loader for deployment-version-registry.toml.
library VersionRegistryConfigLoader {
    Vm private constant VM = Vm(0x7109709ECfa91a80626fF3989D68f67F5b1DD12D);

    string constant VERSION_REGISTRY_CONFIG = "contracts/deployment-version-registry.toml";

    function loadConfig() internal view returns (VersionRegistryConfig memory) {
        string memory configPath = string.concat(VM.projectRoot(), "/", VERSION_REGISTRY_CONFIG);
        string memory config = VM.readFile(configPath);

        // Resolve deploy key from CHAIN_KEY + optional STACK_TAG
        string memory chainKey = VM.envOr("CHAIN_KEY", string(""));
        string memory stackTag = VM.envOr("STACK_TAG", string(""));
        string memory deployKey;
        if (bytes(stackTag).length == 0) {
            deployKey = chainKey;
        } else if (bytes(chainKey).length != 0) {
            deployKey = string.concat(chainKey, "-", stackTag);
        }

        // If no key set, find by chain ID
        if (bytes(deployKey).length == 0) {
            string[] memory deployKeys = VM.parseTomlKeys(config, ".deployment");
            for (uint256 i = 0; i < deployKeys.length; i++) {
                if (stdToml.readUint(config, string.concat(".deployment.", deployKeys[i], ".id")) == block.chainid) {
                    require(bytes(deployKey).length == 0, "multiple entries found with same chain ID");
                    deployKey = deployKeys[i];
                }
            }
        }

        console2.log("Using version-registry deployment key: %s", deployKey);

        string memory chain = string.concat(".deployment.", deployKey);

        VersionRegistryConfig memory cfg;
        cfg.name = stdToml.readString(config, string.concat(chain, ".name"));
        cfg.chainId = stdToml.readUint(config, string.concat(chain, ".id"));
        cfg.admin = stdToml.readAddressOr(config, string.concat(chain, ".admin"), address(0));
        cfg.versionRegistry = stdToml.readAddressOr(config, string.concat(chain, ".version-registry"), address(0));
        cfg.versionRegistryImpl =
            stdToml.readAddressOr(config, string.concat(chain, ".version-registry-impl"), address(0));
        cfg.minimumBrokerVersion =
            stdToml.readStringOr(config, string.concat(chain, ".minimum-broker-version"), "0.0.0");
        cfg.notice = stdToml.readStringOr(config, string.concat(chain, ".notice"), "");

        return cfg;
    }
}

/// @notice Helper to update deployment-version-registry.toml via FFI.
library VersionRegistryTomlUpdater {
    Vm private constant VM = Vm(0x7109709ECfa91a80626fF3989D68f67F5b1DD12D);

    function updateAddresses(address proxy, address impl) internal {
        string[] memory args = new string[](6);
        args[0] = "python3";
        args[1] = "contracts/update_deployment_version_registry_toml.py";
        args[2] = "--version-registry";
        args[3] = Strings.toHexString(proxy);
        args[4] = "--version-registry-impl";
        args[5] = Strings.toHexString(impl);
        VM.ffi(args);
    }

    function updateImpl(address impl) internal {
        string[] memory args = new string[](4);
        args[0] = "python3";
        args[1] = "contracts/update_deployment_version_registry_toml.py";
        args[2] = "--version-registry-impl";
        args[3] = Strings.toHexString(impl);
        VM.ffi(args);
    }

    function updateVersion(string memory version) internal {
        string[] memory args = new string[](4);
        args[0] = "python3";
        args[1] = "contracts/update_deployment_version_registry_toml.py";
        args[2] = "--minimum-broker-version";
        args[3] = version;
        VM.ffi(args);
    }

    function updateNotice(string memory noticeText) internal {
        string[] memory args = new string[](4);
        args[0] = "python3";
        args[1] = "contracts/update_deployment_version_registry_toml.py";
        args[2] = "--notice";
        args[3] = noticeText;
        VM.ffi(args);
    }
}

/// @notice Deployment script for the VersionRegistry contract.
/// @dev Set values in deployment-version-registry.toml to configure the deployment.
contract DeployVersionRegistry is BoundlessScriptBase {
    function run() external {
        VersionRegistryConfig memory cfg = VersionRegistryConfigLoader.loadConfig();

        address versionRegistryAdmin = BoundlessScript.requireLib(cfg.admin, "VersionRegistry admin");

        bytes32 salt = bytes32(uint256(keccak256("VersionRegistry2")));

        vm.startBroadcast(getDeployer());

        address versionRegistryImpl = address(new VersionRegistry{salt: salt}());
        console2.log("Deployed VersionRegistry impl to", versionRegistryImpl);

        address versionRegistryAddress = address(
            new ERC1967Proxy{salt: salt}(
                versionRegistryImpl, abi.encodeCall(VersionRegistry.initialize, (versionRegistryAdmin))
            )
        );
        console2.log("Deployed VersionRegistry proxy to", versionRegistryAddress);
        console2.log("VersionRegistry admin:", versionRegistryAdmin);

        vm.stopBroadcast();

        VersionRegistryTomlUpdater.updateAddresses(versionRegistryAddress, versionRegistryImpl);

        console2.log("VersionRegistry deployed successfully!");
        console2.log("VersionRegistry:", versionRegistryAddress);

        checkUncommittedChangesWarning("Deployment");
    }
}

/// @notice Upgrade script for the VersionRegistry contract.
/// @dev Set values in deployment-version-registry.toml to configure the upgrade.
contract UpgradeVersionRegistry is BoundlessScriptBase {
    function run() external {
        bool gnosisExecute = vm.envOr("GNOSIS_EXECUTE", false);
        bool skipSafetyChecks = vm.envOr("SKIP_SAFETY_CHECKS", false);

        VersionRegistryConfig memory cfg = VersionRegistryConfigLoader.loadConfig();

        address versionRegistryAddress = BoundlessScript.requireLib(cfg.versionRegistry, "version-registry");

        VersionRegistry registry = VersionRegistry(versionRegistryAddress);
        address currentAdmin = registry.owner();
        address currentImplementation = Upgrades.getImplementationAddress(versionRegistryAddress);

        UpgradeOptions memory opts;
        if (skipSafetyChecks) {
            console2.log("WARNING: Skipping all upgrade safety checks!");
            opts.unsafeSkipAllChecks = true;
        } else {
            opts.referenceContract = "build-info-reference:VersionRegistry";
            opts.referenceBuildInfoDir = "contracts/build-info-reference";
        }

        vm.startBroadcast(getDeployer());

        if (gnosisExecute) {
            console2.log("GNOSIS_EXECUTE=true: Deploying new implementation for Safe upgrade");
            console2.log("Target proxy address: ", versionRegistryAddress);
            console2.log("Current implementation: ", currentImplementation);

            address newImpl = Upgrades.prepareUpgrade("VersionRegistry.sol:VersionRegistry", opts);
            console2.log("New implementation deployed: ", newImpl);

            _printGnosisSafeInfo(versionRegistryAddress, newImpl, "");
        } else {
            console2.log("Upgrading VersionRegistry at: ", versionRegistryAddress);
            console2.log("Current implementation: ", currentImplementation);

            Upgrades.upgradeProxy(versionRegistryAddress, "VersionRegistry.sol:VersionRegistry", "", opts, currentAdmin);

            address newImpl = Upgrades.getImplementationAddress(versionRegistryAddress);
            console2.log("Upgraded VersionRegistry implementation to: ", newImpl);

            require(registry.owner() == currentAdmin, "VersionRegistry owner changed during upgrade");

            console2.log("Upgraded VersionRegistry admin is %s", currentAdmin);
            console2.log("Upgraded VersionRegistry proxy contract at %s", versionRegistryAddress);
            console2.log("Upgraded VersionRegistry impl from %s to %s", currentImplementation, newImpl);

            VersionRegistryTomlUpdater.updateImpl(newImpl);
        }

        vm.stopBroadcast();

        checkUncommittedChangesWarning("Upgrade");
    }
}

/// @notice Script for setting the minimum broker version on the VersionRegistry.
/// @dev Reads version from deployment-version-registry.toml. Env vars MAJOR, MINOR, PATCH override.
contract SetMinimumBrokerVersion is BoundlessScriptBase {
    function run() external {
        bool gnosisExecute = vm.envOr("GNOSIS_EXECUTE", false);

        VersionRegistryConfig memory cfg = VersionRegistryConfigLoader.loadConfig();

        address versionRegistryAddress = BoundlessScript.requireLib(cfg.versionRegistry, "version-registry");

        // Parse version: env vars override, then fall back to TOML
        uint16 major;
        uint16 minor;
        uint16 patch;

        uint256 envMajor = vm.envOr("MAJOR", type(uint256).max);
        if (envMajor != type(uint256).max) {
            // Env vars provided — use them
            major = uint16(envMajor);
            minor = uint16(vm.envUint("MINOR"));
            patch = uint16(vm.envUint("PATCH"));
        } else {
            // Parse "major.minor.patch" from TOML
            string memory ver = BoundlessScript.requireLib(cfg.minimumBrokerVersion, "minimum-broker-version");
            (major, minor, patch) = _parseSemver(ver);
        }

        VersionRegistry registry = VersionRegistry(versionRegistryAddress);

        console2.log("VersionRegistry: ", versionRegistryAddress);
        console2.log("Setting minimum broker version to %s.%s.%s", uint256(major), uint256(minor), uint256(patch));

        if (gnosisExecute) {
            console2.log("GNOSIS_EXECUTE=true: Preparing calldata for Safe execution");

            bytes memory callData = abi.encodeCall(VersionRegistry.setMinimumBrokerVersionSemver, (major, minor, patch));

            console2.log("================================");
            console2.log("=== GNOSIS SAFE SET MIN VERSION INFO ===");
            console2.log("Target Address (To): ", versionRegistryAddress);
            console2.log("Function: setMinimumBrokerVersionSemver(uint16,uint16,uint16)");
            console2.log("Calldata:");
            console2.logBytes(callData);
            console2.log("================================");
            console2.log("Transaction NOT executed - use Gnosis Safe to execute");
        } else {
            vm.broadcast(getDeployer());
            registry.setMinimumBrokerVersionSemver(major, minor, patch);

            // Verify
            (uint16 setMajor, uint16 setMinor, uint16 setPatch) = registry.minimumBrokerVersionSemver();
            console2.log(
                "Verified minimum broker version: %s.%s.%s", uint256(setMajor), uint256(setMinor), uint256(setPatch)
            );
        }

        // Write back to TOML
        string memory versionStr =
            string.concat(Strings.toString(major), ".", Strings.toString(minor), ".", Strings.toString(patch));
        VersionRegistryTomlUpdater.updateVersion(versionStr);
    }

    /// @notice Parse a "major.minor.patch" semver string into components.
    function _parseSemver(string memory ver) internal pure returns (uint16 major, uint16 minor, uint16 patch) {
        bytes memory b = bytes(ver);
        uint256 part = 0;
        uint256[3] memory parts;

        for (uint256 i = 0; i < b.length; i++) {
            if (b[i] == ".") {
                part++;
                require(part < 3, "invalid semver: too many dots");
            } else {
                require(uint8(b[i]) >= 0x30 && uint8(b[i]) <= 0x39, "invalid semver: non-digit");
                parts[part] = parts[part] * 10 + (uint8(b[i]) - 0x30);
            }
        }
        require(part == 2, "invalid semver: expected major.minor.patch");

        major = uint16(parts[0]);
        minor = uint16(parts[1]);
        patch = uint16(parts[2]);
    }
}

/// @notice Script for setting the governance notice on the VersionRegistry.
/// @dev Reads notice from deployment-version-registry.toml. Env var NOTICE overrides.
contract SetNotice is BoundlessScriptBase {
    function run() external {
        bool gnosisExecute = vm.envOr("GNOSIS_EXECUTE", false);

        VersionRegistryConfig memory cfg = VersionRegistryConfigLoader.loadConfig();

        address versionRegistryAddress = BoundlessScript.requireLib(cfg.versionRegistry, "version-registry");

        // Env var overrides TOML. Use sentinel to distinguish "not set" from "empty string".
        string memory sentinel = "__UNSET__";
        string memory noticeText = vm.envOr("NOTICE", sentinel);
        if (keccak256(bytes(noticeText)) == keccak256(bytes(sentinel))) {
            noticeText = cfg.notice;
        }

        VersionRegistry registry = VersionRegistry(versionRegistryAddress);

        console2.log("VersionRegistry: ", versionRegistryAddress);
        if (bytes(noticeText).length == 0) {
            console2.log("Clearing notice (setting to empty string)");
        } else {
            console2.log("Setting notice to: %s", noticeText);
        }

        if (gnosisExecute) {
            console2.log("GNOSIS_EXECUTE=true: Preparing calldata for Safe execution");

            bytes memory callData = abi.encodeCall(VersionRegistry.setNotice, (noticeText));

            console2.log("================================");
            console2.log("=== GNOSIS SAFE SET NOTICE INFO ===");
            console2.log("Target Address (To): ", versionRegistryAddress);
            console2.log("Function: setNotice(string)");
            console2.log("Calldata:");
            console2.logBytes(callData);
            console2.log("================================");
            console2.log("Transaction NOT executed - use Gnosis Safe to execute");
        } else {
            vm.broadcast(getDeployer());
            registry.setNotice(noticeText);

            string memory currentNotice = registry.notice();
            console2.log("Verified notice: %s", currentNotice);
        }

        // Write back to TOML
        VersionRegistryTomlUpdater.updateNotice(noticeText);
    }
}

/// @notice Script for accepting ownership of the VersionRegistry (Ownable2Step step 2).
/// @dev Must be called by the pending owner set via TransferVersionRegistryOwnership.
contract AcceptVersionRegistryOwnership is BoundlessScriptBase {
    function run() external {
        bool gnosisExecute = vm.envOr("GNOSIS_EXECUTE", false);

        VersionRegistryConfig memory cfg = VersionRegistryConfigLoader.loadConfig();

        address versionRegistryAddress = BoundlessScript.requireLib(cfg.versionRegistry, "version-registry");

        VersionRegistry registry = VersionRegistry(versionRegistryAddress);
        address pendingOwner = registry.pendingOwner();
        address currentOwner = registry.owner();

        console2.log("VersionRegistry: ", versionRegistryAddress);
        console2.log("Current owner: ", currentOwner);
        console2.log("Pending owner: ", pendingOwner);
        require(pendingOwner != address(0), "No pending ownership transfer");

        if (gnosisExecute) {
            console2.log("GNOSIS_EXECUTE=true: Preparing calldata for Safe execution");

            bytes memory callData = abi.encodeCall(registry.acceptOwnership, ());

            console2.log("================================");
            console2.log("=== GNOSIS SAFE ACCEPT OWNERSHIP INFO ===");
            console2.log("Target Address (To): ", versionRegistryAddress);
            console2.log("Function: acceptOwnership()");
            console2.log("Calldata:");
            console2.logBytes(callData);
            console2.log("================================");
            console2.log("Transaction NOT executed - use Gnosis Safe to execute");
        } else {
            vm.broadcast(getDeployer());
            registry.acceptOwnership();

            console2.log("Ownership accepted. New owner: ", registry.owner());
        }
    }
}

/// @notice Script for transferring ownership of the VersionRegistry (Ownable2Step step 1).
/// @dev Requires NEW_OWNER environment variable. New owner must call AcceptVersionRegistryOwnership to complete.
contract TransferVersionRegistryOwnership is BoundlessScriptBase {
    function run() external {
        bool gnosisExecute = vm.envOr("GNOSIS_EXECUTE", false);

        VersionRegistryConfig memory cfg = VersionRegistryConfigLoader.loadConfig();

        address versionRegistryAddress = BoundlessScript.requireLib(cfg.versionRegistry, "version-registry");

        address newOwner = vm.envAddress("NEW_OWNER");
        require(newOwner != address(0), "NEW_OWNER environment variable not set or zero address");

        VersionRegistry registry = VersionRegistry(versionRegistryAddress);
        address currentOwner = registry.owner();

        console2.log("VersionRegistry: ", versionRegistryAddress);
        console2.log("Current owner: ", currentOwner);
        console2.log("New owner (pending): ", newOwner);
        console2.log("NOTE: New owner must call acceptOwnership() to complete the transfer (Ownable2Step)");

        if (gnosisExecute) {
            console2.log("GNOSIS_EXECUTE=true: Preparing calldata for Safe execution");

            bytes memory callData = abi.encodeCall(registry.transferOwnership, (newOwner));

            console2.log("================================");
            console2.log("=== GNOSIS SAFE TRANSFER OWNERSHIP INFO ===");
            console2.log("Target Address (To): ", versionRegistryAddress);
            console2.log("Function: transferOwnership(address)");
            console2.log("New Owner: ", newOwner);
            console2.log("Calldata:");
            console2.logBytes(callData);
            console2.log("================================");
            console2.log("Transaction NOT executed - use Gnosis Safe to execute");
        } else {
            vm.broadcast(getDeployer());
            registry.transferOwnership(newOwner);

            console2.log("Ownership transfer initiated. Pending owner: ", registry.pendingOwner());
            console2.log("New owner must call acceptOwnership() to complete the transfer.");
        }
    }
}
