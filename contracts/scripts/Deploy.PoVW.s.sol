// Copyright 2025 RISC Zero, Inc.
//
// Use of this source code is governed by the Business Source License
// as found in the LICENSE-BSL file.
// SPDX-License-Identifier: BUSL-1.1

pragma solidity ^0.8.9;

import {Script, console2} from "forge-std/Script.sol";
import {IRiscZeroVerifier} from "risc0/IRiscZeroVerifier.sol";
import {IRiscZeroSelectable} from "risc0/IRiscZeroSelectable.sol";
import {RiscZeroVerifierRouter} from "risc0/RiscZeroVerifierRouter.sol";
import {RiscZeroSetVerifier} from "risc0/RiscZeroSetVerifier.sol";
import {RiscZeroCheats} from "risc0/test/RiscZeroCheats.sol";
import {PovwAccounting} from "../src/povw/PovwAccounting.sol";
import {PovwMint} from "../src/povw/PovwMint.sol";
import {IZKC, IZKCRewards} from "../src/povw/IZKC.sol";
import {MockZKC, MockZKCRewards} from "../test/MockZKC.sol";
import {ConfigLoader, DeploymentConfig} from "./Config.s.sol";
import {ImageID} from "../src/libraries/PovwImageId.sol";
import {ERC1967Proxy} from "@openzeppelin/contracts/proxy/ERC1967/ERC1967Proxy.sol";

contract DeployPoVW is Script, RiscZeroCheats {
    // Path to deployment config file, relative to the project root.
    string constant CONFIG_FILE = "contracts/deployment.toml";

    /// @notice Gets the current git commit hash from environment variable.
    function getCurrentCommit() internal view returns (string memory) {
        string memory commit = vm.envOr("CURRENT_COMMIT", string("unknown"));
        return commit;
    }

    function run() external {
        // load ENV variables first
        uint256 deployerKey = vm.envOr("DEPLOYER_PRIVATE_KEY", uint256(0));
        require(deployerKey != 0, "No deployer key provided. Please set the env var DEPLOYER_PRIVATE_KEY. Ensure private key prefixed with 0x");
        vm.rememberKey(deployerKey);

        console2.log("Deploying PoVW contracts (admins will be loaded from deployment.toml)");

        // Read and log the chainID
        uint256 chainId = block.chainid;
        console2.log("You are deploying on ChainID %d", chainId);

        // Load the deployment config
        DeploymentConfig memory deploymentConfig =
            ConfigLoader.loadDeploymentConfig(string.concat(vm.projectRoot(), "/", CONFIG_FILE));

        // Validate admin addresses are set (use deployment config instead of env var)
        address povwAccountingAdmin = deploymentConfig.povwAccountingAdmin;
        address povwMintAdmin = deploymentConfig.povwMintAdmin;
        require(povwAccountingAdmin != address(0), "PovwAccounting admin address must be set in deployment.toml");
        require(povwMintAdmin != address(0), "PovwMint admin address must be set in deployment.toml");

        IRiscZeroVerifier verifier;
        bool devMode = bytes(vm.envOr("RISC0_DEV_MODE", string(""))).length > 0;
        
        if (!devMode) {
            verifier = IRiscZeroVerifier(deploymentConfig.verifier);
            require(address(verifier) != address(0), "Verifier must be deployed first or RISC0_DEV_MODE must be set");
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
        address zkcRewardsAddress;
        
        if (devMode) {
            // Deploy mock ZKC contracts only in dev mode
            MockZKC mockZKC = new MockZKC();
            MockZKCRewards mockZKCRewards = new MockZKCRewards();
            
            zkcAddress = address(mockZKC);
            zkcRewardsAddress = address(mockZKCRewards);
            
            console2.log("In DEV MODE. Redeploying Mock ZKC and Mock ZKCRewards");
            console2.log("Deployed MockZKC to", zkcAddress);
            console2.log("Deployed MockZKCRewards to", zkcRewardsAddress);
        } else if (deploymentConfig.zkc != address(0) && deploymentConfig.zkcStakingRewards != address(0)) {
            // Use existing ZKC contracts
            zkcAddress = deploymentConfig.zkc;
            zkcRewardsAddress = deploymentConfig.zkcStakingRewards;
            console2.log("Using existing ZKC at", zkcAddress);
            console2.log("Using existing ZKCStakingRewards at", zkcRewardsAddress);
        } else {
            revert("ZKC contracts must be specified in deployment.toml, or RISC0_DEV_MODE must be set");
        }

        // PoVW image IDs from solidity library (use mock values in dev mode)
        bytes32 logUpdaterId;
        bytes32 mintCalculatorId;
        
        if (devMode) {
            // Use mock image IDs when in dev mode
            logUpdaterId = bytes32(uint256(0x1111111111111111111111111111111111111111111111111111111111111111));
            mintCalculatorId = bytes32(uint256(0x2222222222222222222222222222222222222222222222222222222222222222));
            console2.log("Using mock PoVW image IDs for dev mode");
        } else {
            // In production, use image IDs from environment variables if set, otherwise from solidity library
            logUpdaterId = vm.envOr("POVW_LOG_UPDATER_ID", ImageID.BOUNDLESS_POVW_LOG_UPDATER_ID);
            mintCalculatorId = vm.envOr("POVW_MINT_CALCULATOR_ID", ImageID.BOUNDLESS_POVW_MINT_CALCULATOR_ID);
            
            if (vm.envOr("POVW_LOG_UPDATER_ID", bytes32(0)) != bytes32(0) || 
                vm.envOr("POVW_MINT_CALCULATOR_ID", bytes32(0)) != bytes32(0)) {
                console2.log("Using PoVW image IDs from environment variables (overriding PovwImageId.sol)");
            } else {
                console2.log("Using production PoVW image IDs from PovwImageId.sol");
            }
        }
        
        console2.log("Log Updater ID: %s", vm.toString(logUpdaterId));
        console2.log("Mint Calculator ID: %s", vm.toString(mintCalculatorId));

        // Deploy PovwAccounting
        bytes32 salt = bytes32(0);
        address povwAccountingImpl = address(
            new PovwAccounting{salt: salt}(verifier, IZKC(zkcAddress), logUpdaterId)
        );
        address povwAccountingAddress = address(
            new ERC1967Proxy{salt: salt}(
                povwAccountingImpl, 
                abi.encodeCall(PovwAccounting.initialize, (povwAccountingAdmin))
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
                IZKCRewards(zkcRewardsAddress)
            )
        );
        address povwMintAddress = address(
            new ERC1967Proxy{salt: salt}(
                povwMintImpl,
                abi.encodeCall(PovwMint.initialize, (povwMintAdmin))
            )
        );

        console2.log("Deployed PovwMint impl to", povwMintImpl);
        console2.log("Deployed PovwMint proxy to", povwMintAddress);
        console2.log("PovwMint admin:", povwMintAdmin);

        vm.stopBroadcast();

        // Update deployment.toml with contract addresses and image IDs
        console2.log("Updating deployment.toml with PoVW contract addresses and image IDs");
        
        // Get current git commit hash
        string memory currentCommit = getCurrentCommit();

        string[] memory args = new string[](26);
        args[0] = "python3";
        args[1] = "contracts/update_deployment_toml.py";
        args[2] = "--povw-accounting";
        args[3] = vm.toString(povwAccountingAddress);
        args[4] = "--povw-accounting-impl";
        args[5] = vm.toString(povwAccountingImpl);
        args[6] = "--povw-mint";
        args[7] = vm.toString(povwMintAddress);
        args[8] = "--povw-mint-impl";
        args[9] = vm.toString(povwMintImpl);
        args[10] = "--povw-mint-old-impl";
        args[11] = vm.toString(address(0));
        args[12] = "--povw-accounting-old-impl";
        args[13] = vm.toString(address(0));
        args[14] = "--povw-log-updater-id";
        args[15] = vm.toString(logUpdaterId);
        args[16] = "--povw-mint-calculator-id";
        args[17] = vm.toString(mintCalculatorId);
        args[18] = "--povw-accounting-deployment-commit";
        args[19] = currentCommit;
        args[20] = "--povw-mint-deployment-commit";
        args[21] = currentCommit;
        args[22] = "--zkc";
        args[23] = vm.toString(zkcAddress);
        args[24] = "--zkc-staking-rewards";
        args[25] = vm.toString(zkcRewardsAddress);
        vm.ffi(args);
        console2.log("Deployment.toml updated with PoVW contract addresses, image IDs, and commit: %s", currentCommit);

        console2.log("PoVW contracts deployed successfully!");
        console2.log("ZKC:", zkcAddress);
        console2.log("ZKCStakingRewards:", zkcRewardsAddress);
        console2.log("PovwAccounting:", povwAccountingAddress);
        console2.log("PovwMint:", povwMintAddress);
        
        if (devMode) {
            console2.log("");
            console2.log("WARNING: RISC0_DEV_MODE was enabled - deployed with mock verifier, ZKC contracts, and test image IDs");
        }
        
        // Check for uncommitted changes warning
        string memory hasUnstaged = vm.envOr("HAS_UNSTAGED_CHANGES", string(""));
        string memory hasStaged = vm.envOr("HAS_STAGED_CHANGES", string(""));
        if (bytes(hasUnstaged).length > 0 || bytes(hasStaged).length > 0) {
            console2.log("WARNING: Deployment was done with uncommitted changes!");
        }
    }
}