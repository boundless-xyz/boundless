// Copyright 2026 Boundless Foundation, Inc.
//
// Use of this source code is governed by the Business Source License
// as found in the LICENSE-BSL file.
// SPDX-License-Identifier: BUSL-1.1

pragma solidity ^0.8.26;

import {console2} from "forge-std/Script.sol";
import {IRiscZeroVerifier} from "risc0/IRiscZeroVerifier.sol";
import {IRiscZeroSelectable} from "risc0/IRiscZeroSelectable.sol";
import {RiscZeroVerifierRouter} from "risc0/RiscZeroVerifierRouter.sol";
import {RiscZeroSetVerifier} from "risc0/RiscZeroSetVerifier.sol";
import {RiscZeroCheats} from "risc0/test/RiscZeroCheats.sol";
import {PovwAccounting} from "../src/povw/PovwAccounting.sol";
import {PovwMint} from "../src/povw/PovwMint.sol";
import {IZKC} from "zkc/interfaces/IZKC.sol";
import {IRewards as IZKCRewards} from "zkc/interfaces/IRewards.sol";
import {MockZKC, MockZKCRewards} from "../test/MockZKC.sol";
import {ConfigLoader, DeploymentConfig} from "./Config.s.sol";
import {ERC1967Proxy} from "@openzeppelin/contracts/proxy/ERC1967/ERC1967Proxy.sol";
import {BoundlessScriptBase, BoundlessScript} from "./BoundlessScript.s.sol";

contract DeployPoVW is BoundlessScriptBase, RiscZeroCheats {
    struct DeployedContracts {
        address verifier;
        address zkc;
        address vezkc;
        address povwAccountingImpl;
        address povwAccountingAddress;
        address povwMintImpl;
        address povwMintAddress;
        bytes32 logUpdaterId;
        bytes32 mintCalculatorId;
    }

    /// @notice Updates deployment.toml with deployed contract addresses and image IDs
    function updateDeploymentToml(DeployedContracts memory contracts) internal {
        console2.log("Updating deployment.toml with PoVW contract addresses and image IDs");

        // Get current git commit hash
        string memory currentCommit = getCurrentCommit();

        string[] memory args = new string[](28);
        args[0] = "python3";
        args[1] = "contracts/update_deployment_toml.py";
        args[2] = "--povw-accounting";
        args[3] = vm.toString(contracts.povwAccountingAddress);
        args[4] = "--povw-accounting-impl";
        args[5] = vm.toString(contracts.povwAccountingImpl);
        args[6] = "--povw-mint";
        args[7] = vm.toString(contracts.povwMintAddress);
        args[8] = "--povw-mint-impl";
        args[9] = vm.toString(contracts.povwMintImpl);
        args[10] = "--povw-mint-old-impl";
        args[11] = vm.toString(address(0));
        args[12] = "--povw-accounting-old-impl";
        args[13] = vm.toString(address(0));
        args[14] = "--povw-log-updater-id";
        args[15] = vm.toString(contracts.logUpdaterId);
        args[16] = "--povw-mint-calculator-id";
        args[17] = vm.toString(contracts.mintCalculatorId);
        args[18] = "--povw-accounting-deployment-commit";
        args[19] = currentCommit;
        args[20] = "--povw-mint-deployment-commit";
        args[21] = currentCommit;
        args[22] = "--zkc";
        args[23] = vm.toString(contracts.zkc);
        args[24] = "--vezkc";
        args[25] = vm.toString(contracts.vezkc);
        args[26] = "--verifier";
        args[27] = vm.toString(contracts.verifier);
        vm.ffi(args);
    }

    function run() external {
        // load ENV variables first
        uint256 deployerKey = vm.envOr("DEPLOYER_PRIVATE_KEY", uint256(0));
        require(
            deployerKey != 0,
            "No deployer key provided. Please set the env var DEPLOYER_PRIVATE_KEY. Ensure private key prefixed with 0x"
        );
        vm.rememberKey(deployerKey);

        console2.log("Deploying PoVW contracts (admins will be loaded from deployment.toml)");

        // Read and log the chainID
        uint256 chainId = block.chainid;
        console2.log("You are deploying on ChainID %d", chainId);

        // Load the deployment config
        DeploymentConfig memory deploymentConfig =
            ConfigLoader.loadDeploymentConfig(string.concat(vm.projectRoot(), "/", CONFIG));

        // Validate admin addresses are set (use deployment config instead of env var)
        address povwAccountingAdmin =
            BoundlessScript.requireLib(deploymentConfig.povwAccountingAdmin, "PovwAccounting admin");
        address povwMintAdmin = BoundlessScript.requireLib(deploymentConfig.povwMintAdmin, "PovwMint admin");

        IRiscZeroVerifier verifier;
        bool devMode = bytes(vm.envOr("RISC0_DEV_MODE", string(""))).length > 0;

        if (!devMode) {
            verifier = IRiscZeroVerifier(BoundlessScript.requireLib(deploymentConfig.verifier, "Verifier"));
            console2.log("Using IRiscZeroVerifier at", address(verifier));
        }

        vm.startBroadcast();

        if (devMode) {
            // Deploy verifier in dev mode
            RiscZeroVerifierRouter verifierRouter = new RiscZeroVerifierRouter(povwAccountingAdmin);
            console2.log("Deployed RiscZeroVerifierRouter to", address(verifierRouter));

            IRiscZeroVerifier _verifier = deployRiscZeroVerifier();
            IRiscZeroSelectable selectable = IRiscZeroSelectable(address(_verifier));
            bytes4 selector = selectable.SELECTOR();
            verifierRouter.addVerifier(selector, _verifier);

            // Deploy set verifier for dev mode
            string memory setBuilderPath =
                "/target/riscv-guest/guest-set-builder/set-builder/riscv32im-risc0-zkvm-elf/release/set-builder.bin";
            string memory cwd = vm.envString("PWD");
            string memory setBuilderGuestUrl = string.concat("file://", cwd, setBuilderPath);
            console2.log("Set builder URI", setBuilderGuestUrl);

            string[] memory argv = new string[](4);
            argv[0] = "r0vm";
            argv[1] = "--id";
            argv[2] = "--elf";
            argv[3] = string.concat(".", setBuilderPath);
            bytes32 setBuilderImageId = abi.decode(vm.ffi(argv), (bytes32));

            RiscZeroSetVerifier setVerifier =
                new RiscZeroSetVerifier(IRiscZeroVerifier(verifierRouter), setBuilderImageId, setBuilderGuestUrl);
            console2.log("Deployed RiscZeroSetVerifier to", address(setVerifier));
            verifierRouter.addVerifier(setVerifier.SELECTOR(), setVerifier);

            verifier = IRiscZeroVerifier(verifierRouter);
            console2.log("Dev mode: Deployed RiscZeroVerifier at", address(verifier));
        }

        // Determine ZKC contracts to use - deploy mocks only in RISC0_DEV_MODE
        address zkcAddress;
        address vezkcAddress;

        if (devMode) {
            // Deploy mock ZKC contracts only in dev mode
            MockZKC mockZkc = new MockZKC();
            MockZKCRewards mockZkcRewards = new MockZKCRewards();

            zkcAddress = address(mockZkc);
            vezkcAddress = address(mockZkcRewards);

            console2.log("In DEV MODE. Redeploying Mock ZKC and Mock ZKCRewards");
            console2.log("Deployed MockZKC to", zkcAddress);
            console2.log("Deployed MockZKCRewards to", vezkcAddress);
        } else {
            // Use existing ZKC contracts
            zkcAddress = BoundlessScript.requireLib(deploymentConfig.zkc, "ZKC");
            vezkcAddress = BoundlessScript.requireLib(deploymentConfig.vezkc, "veZKC");
            console2.log("Using existing ZKC at", zkcAddress);
            console2.log("Using existing veZKC at", vezkcAddress);
        }

        // PoVW image IDs (use mock values in dev mode)
        bytes32 logUpdaterId;
        bytes32 mintCalculatorId;

        if (devMode) {
            // Use mock image IDs when in dev mode
            logUpdaterId = bytes32(uint256(0x1111111111111111111111111111111111111111111111111111111111111111));
            mintCalculatorId = bytes32(uint256(0x2222222222222222222222222222222222222222222222222222222222222222));
            console2.log("Using mock PoVW image IDs for dev mode");
        } else {
            // Check if environment variables are set first
            bytes32 envLogUpdater = vm.envOr("POVW_LOG_UPDATER_ID", bytes32(0));
            bytes32 envMintCalculator = vm.envOr("POVW_MINT_CALCULATOR_ID", bytes32(0));

            if (envLogUpdater != bytes32(0) && envMintCalculator != bytes32(0)) {
                // Use environment variables if both are set
                logUpdaterId = envLogUpdater;
                mintCalculatorId = envMintCalculator;
                console2.log("Using PoVW image IDs from environment variables");
            } else {
                // Use .bin files as default
                logUpdaterId = readImageIdFromFile("boundless-povw-log-updater.bin");
                mintCalculatorId = readImageIdFromFile("boundless-povw-mint-calculator.bin");
                console2.log("Using PoVW image IDs from .bin files");
            }

            // Require that we have valid image IDs
            logUpdaterId = BoundlessScript.requireLib(logUpdaterId, "Log Updater ID");
            mintCalculatorId = BoundlessScript.requireLib(mintCalculatorId, "Mint Calculator ID");
        }

        console2.log("Log Updater ID: %s", vm.toString(logUpdaterId));
        console2.log("Mint Calculator ID: %s", vm.toString(mintCalculatorId));

        // Deploy PovwAccounting
        bytes32 salt = bytes32(vm.envOr("SALT", uint256(0)));
        address povwAccountingImpl = address(new PovwAccounting{salt: salt}(verifier, IZKC(zkcAddress), logUpdaterId));
        address povwAccountingAddress = address(
            new ERC1967Proxy{salt: salt}(
                povwAccountingImpl, abi.encodeCall(PovwAccounting.initialize, (povwAccountingAdmin))
            )
        );

        console2.log("Deployed PovwAccounting impl to", povwAccountingImpl);
        console2.log("Deployed PovwAccounting proxy to", povwAccountingAddress);
        console2.log("PovwAccounting admin:", povwAccountingAdmin);

        // Deploy PovwMint
        address povwMintImpl = address(
            new PovwMint{salt: salt}(
                verifier,
                PovwAccounting(povwAccountingAddress),
                mintCalculatorId,
                IZKC(zkcAddress),
                IZKCRewards(vezkcAddress)
            )
        );
        address povwMintAddress =
            address(new ERC1967Proxy{salt: salt}(povwMintImpl, abi.encodeCall(PovwMint.initialize, (povwMintAdmin))));

        console2.log("Deployed PovwMint impl to", povwMintImpl);
        console2.log("Deployed PovwMint proxy to", povwMintAddress);
        console2.log("PovwMint admin:", povwMintAdmin);

        vm.stopBroadcast();

        // Update deployment.toml with contract addresses and image IDs
        DeployedContracts memory deployedContracts = DeployedContracts({
            verifier: address(verifier),
            zkc: zkcAddress,
            vezkc: vezkcAddress,
            povwAccountingImpl: povwAccountingImpl,
            povwAccountingAddress: povwAccountingAddress,
            povwMintImpl: povwMintImpl,
            povwMintAddress: povwMintAddress,
            logUpdaterId: logUpdaterId,
            mintCalculatorId: mintCalculatorId
        });
        updateDeploymentToml(deployedContracts);

        console2.log("PoVW contracts deployed successfully!");
        console2.log("ZKC:", zkcAddress);
        console2.log("veZKC:", vezkcAddress);
        console2.log("PovwAccounting:", povwAccountingAddress);
        console2.log("PovwMint:", povwMintAddress);

        if (devMode) {
            console2.log("");
            console2.log("=================================================================");
            console2.log("WARNING: RISC0_DEV_MODE was enabled!");
            console2.log("- Deployed with mock verifier, ZKC contracts, and test image IDs");
            console2.log("- deployment.toml was updated with mock addresses");
            console2.log("- These contracts are NOT suitable for production use");
            console2.log("=================================================================");
        }

        // Check for uncommitted changes warning
        checkUncommittedChangesWarning("Deployment");
    }
}
