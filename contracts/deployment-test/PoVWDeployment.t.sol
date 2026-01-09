// Copyright 2026 Boundless Foundation, Inc.
//
// Use of this source code is governed by the Business Source License
// as found in the LICENSE-BSL file.
// SPDX-License-Identifier: BUSL-1.1

pragma solidity ^0.8.26;

import {Test} from "forge-std/Test.sol";
import {Vm} from "forge-std/Vm.sol";
import {IRiscZeroVerifier} from "risc0/IRiscZeroVerifier.sol";

import {PovwAccounting, PendingEpoch} from "../src/povw/PovwAccounting.sol";
import {PovwMint} from "../src/povw/PovwMint.sol";
import {ConfigLoader, DeploymentConfig} from "../scripts/Config.s.sol";

Vm constant VM = Vm(0x7109709ECfa91a80626fF3989D68f67F5b1DD12D);

/// Test designed to be run against a chain with an active deployment of the PoVW contracts.
/// Checks that the deployment matches what is recorded in the deployment.toml file and
/// validates the upgrade functionality and ownership patterns.
contract PoVWDeploymentTest is Test {
    // Path to deployment config file, relative to the project root.
    string constant CONFIG_FILE = "contracts/deployment.toml";
    // Load the deployment config
    DeploymentConfig internal deployment;

    IRiscZeroVerifier internal verifier;
    PovwAccounting internal povwAccounting;
    PovwMint internal povwMint;

    function setUp() external {
        // Load the deployment config
        deployment = ConfigLoader.loadDeploymentConfig(string.concat(vm.projectRoot(), "/", CONFIG_FILE));

        require(deployment.verifier != address(0), "no verifier address is set");
        verifier = IRiscZeroVerifier(deployment.verifier);

        // Load PoVW contract addresses from deployment config
        require(deployment.povwAccounting != address(0), "no PoVW accounting address is set");
        povwAccounting = PovwAccounting(deployment.povwAccounting);
        require(deployment.povwMint != address(0), "no PoVW mint address is set");
        povwMint = PovwMint(deployment.povwMint);
    }

    function testAdminIsSet() external view {
        require(deployment.admin != address(0), "no admin address is set");
    }

    function testVerifierIsDeployed() external view {
        require(address(verifier) != address(0), "no verifier address is set");
        require(keccak256(address(verifier).code) != keccak256(bytes("")), "verifier code is empty");
    }

    function testPovwAccountingIsDeployed() external view {
        require(address(povwAccounting) != address(0), "no PoVW accounting address is set");
        require(keccak256(address(povwAccounting).code) != keccak256(bytes("")), "PoVW accounting code is empty");
    }

    function testPovwMintIsDeployed() external view {
        require(address(povwMint) != address(0), "no PoVW mint address is set");
        require(keccak256(address(povwMint).code) != keccak256(bytes("")), "PoVW mint code is empty");
    }

    function testPovwAccountingOwner() external view {
        require(
            deployment.povwAccountingAdmin == povwAccounting.owner(),
            "PoVW accounting owner does not match configured admin"
        );
    }

    function testPovwMintOwner() external view {
        require(deployment.povwMintAdmin == povwMint.owner(), "PoVW mint owner does not match configured admin");
    }

    function testPovwAccountingAdminIsSet() external view {
        require(deployment.povwAccountingAdmin != address(0), "PovwAccounting admin address should be set");
    }

    function testPovwMintAdminIsSet() external view {
        require(deployment.povwMintAdmin != address(0), "PovwMint admin address should be set");
    }

    function testPovwAccountingVerifier() external view {
        require(
            address(povwAccounting.VERIFIER()) == address(verifier),
            "PoVW accounting verifier does not match deployment verifier"
        );
    }

    function testPovwAccountingLogUpdaterId() external view {
        // The log updater ID should be set to a non-zero value
        require(povwAccounting.LOG_UPDATER_ID() != bytes32(0), "PoVW accounting log updater ID should not be zero");
    }

    function testPovwAccountingIsUpgradeable() external view {
        // Test that the contract has the VERSION constant (indicates upgradeability)
        require(povwAccounting.VERSION() >= 1, "PoVW accounting version should be 1");
    }

    function testPovwMintIsUpgradeable() external view {
        // Test that the contract has the VERSION constant (indicates upgradeability)
        require(povwMint.VERSION() >= 1, "PoVW mint version should be 1");
    }

    function testPovwAccountingPendingEpoch() external view {
        // Get the pending epoch from the accounting contract
        PendingEpoch memory pendingEpoch = povwAccounting.pendingEpoch();

        // Pending epoch should have a reasonable number (not zero)
        require(pendingEpoch.number > 0, "Pending epoch number should be greater than zero");
        // Total work starts at zero
        require(pendingEpoch.totalWork == 0, "Initial total work should be zero");
    }

    function testContractIntegration() external view {
        // Test that both contracts use the same verifier
        require(
            address(povwAccounting.VERIFIER()) == address(verifier),
            "PovwAccounting should use the same verifier as configured"
        );
    }

    function testOwnershipTransferCapability() external view {
        // Verify current owner is the configured admin
        require(
            povwAccounting.owner() == deployment.povwAccountingAdmin,
            "PovwAccounting owner should match configured admin"
        );
        require(povwMint.owner() == deployment.povwMintAdmin, "PovwMint owner should match configured admin");

        // With regular Ownable, ownership transfer is immediate (no pending state)
        // Just verify the owner is correctly set
        require(deployment.povwAccountingAdmin != address(0), "PovwAccounting admin should be set");
        require(deployment.povwMintAdmin != address(0), "PovwMint admin should be set");
    }

    function testWorkLogCommitInitialState() external view {
        // Test getting work log commit for a random address (should be zero initially)
        address testWorkLogId = address(0x1111111111111111111111111111111111111111);
        bytes32 commit = povwAccounting.workLogCommit(testWorkLogId);

        // Should return zero bytes for non-existent work logs
        require(commit == bytes32(0), "Non-existent work log should return zero commit");
    }
}
