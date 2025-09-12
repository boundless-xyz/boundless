// Copyright 2025 RISC Zero, Inc.
//
// Use of this source code is governed by the Business Source License
// as found in the LICENSE-BSL file.

pragma solidity ^0.8.20;

import {Script, console2} from "forge-std/Script.sol";
import {Strings} from "openzeppelin/contracts/utils/Strings.sol";
import {IRiscZeroSelectable} from "risc0/IRiscZeroSelectable.sol";
import {IRiscZeroVerifier} from "risc0/IRiscZeroVerifier.sol";
import {RiscZeroSetVerifier} from "risc0/RiscZeroSetVerifier.sol";
import {RiscZeroVerifierRouter} from "risc0/RiscZeroVerifierRouter.sol";
import {RiscZeroCheats} from "risc0/test/RiscZeroCheats.sol";
import {ERC1967Proxy} from "@openzeppelin/contracts/proxy/ERC1967/ERC1967Proxy.sol";
import {ConfigLoader, DeploymentConfig} from "./Config.s.sol";
import {BoundlessMarket} from "../src/BoundlessMarket.sol";
import {HitPoints} from "../src/HitPoints.sol";
import {BoundlessScriptBase} from "./BoundlessScript.s.sol";

contract Deploy is BoundlessScriptBase, RiscZeroCheats {
    // Path to deployment config file, relative to the project root.
    string constant CONFIG_FILE = "contracts/deployment.toml";

    IRiscZeroVerifier verifier;
    address boundlessMarketAddress;
    bytes32 assessorImageId;
    address stakeToken;

    function run() external {
        string memory assessorGuestUrl = "";

        // load ENV variables first
        uint256 deployerKey = vm.envOr("DEPLOYER_PRIVATE_KEY", uint256(0));
        require(deployerKey != 0, "No deployer key provided. Please set the env var DEPLOYER_PRIVATE_KEY.");
        vm.rememberKey(deployerKey);

        address boundlessMarketOwner = vm.envAddress("BOUNDLESS_MARKET_OWNER");
        console2.log("BoundlessMarket Owner:", boundlessMarketOwner);

        // Read and log the chainID
        uint256 chainId = block.chainid;
        console2.log("You are deploying on ChainID %d", chainId);

        // Load the deployment config
        DeploymentConfig memory deploymentConfig =
            ConfigLoader.loadDeploymentConfig(string.concat(vm.projectRoot(), "/", CONFIG_FILE));

        // Assign parsed config values to the variables
        verifier = IRiscZeroVerifier(deploymentConfig.verifier);
        assessorImageId = deploymentConfig.assessorImageId;
        assessorGuestUrl = deploymentConfig.assessorGuestUrl;

        if (assessorImageId == bytes32(0)) {
            revert("assessor image ID must be set in deployment.toml");
        }

        vm.startBroadcast(deployerKey);

        // Deploy the verifier, if dev mode is enabled.
        if (bytes(vm.envOr("RISC0_DEV_MODE", string(""))).length > 0) {
            RiscZeroVerifierRouter verifierRouter = new RiscZeroVerifierRouter(boundlessMarketOwner);
            console2.log("Deployed RiscZeroVerifierRouter to", address(verifierRouter));

            IRiscZeroVerifier _verifier = deployRiscZeroVerifier();
            IRiscZeroSelectable selectable = IRiscZeroSelectable(address(_verifier));
            bytes4 selector = selectable.SELECTOR();
            verifierRouter.addVerifier(selector, _verifier);

            // TODO: Create a more robust way of getting a URI for guests, and ensure that it is
            // in-sync with the configured image ID.
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

            string memory assessorPath =
                "/target/riscv-guest/guest-assessor/assessor-guest/riscv32im-risc0-zkvm-elf/release/assessor-guest.bin";
            assessorGuestUrl = string.concat("file://", cwd, assessorPath);
            console2.log("Assessor URI", assessorGuestUrl);

            argv[3] = string.concat(".", assessorPath);
            assessorImageId = abi.decode(vm.ffi(argv), (bytes32));

            RiscZeroSetVerifier setVerifier =
                new RiscZeroSetVerifier(IRiscZeroVerifier(verifierRouter), setBuilderImageId, setBuilderGuestUrl);
            console2.log("Deployed RiscZeroSetVerifier to", address(setVerifier));
            verifierRouter.addVerifier(setVerifier.SELECTOR(), setVerifier);

            verifier = IRiscZeroVerifier(verifierRouter);
        }

        if (address(verifier) == address(0)) {
            revert("verifier must be specified in deployment.toml");
        } else {
            console2.log("Using IRiscZeroVerifier deployed at", address(verifier));
        }

        if (deploymentConfig.collateralToken == address(0)) {
            // Deploy the HitPoints contract
            stakeToken = address(new HitPoints(boundlessMarketOwner));
            HitPoints(stakeToken).grantMinterRole(boundlessMarketOwner);
            console2.log("Deployed HitPoints collateral token to", stakeToken);
        } else {
            stakeToken = deploymentConfig.collateralToken;
            console2.log("Using collateral token deployed at", stakeToken);
        }

        // Deploy the Boundless market
        bytes32 salt = vm.envOr("SALT", keccak256(abi.encodePacked("salt")));
        address newImplementation =
            address(new BoundlessMarket{salt: salt}(verifier, assessorImageId, bytes32(0), 0, stakeToken));
        console2.log("Deployed new BoundlessMarket implementation at", newImplementation);
        boundlessMarketAddress = address(
            new ERC1967Proxy{salt: salt}(
                newImplementation, abi.encodeCall(BoundlessMarket.initialize, (boundlessMarketOwner, assessorGuestUrl))
            )
        );
        console2.log("Deployed BoundlessMarket (proxy) to", boundlessMarketAddress);

        if (deploymentConfig.collateralToken == address(0)) {
            HitPoints(stakeToken).grantAuthorizedTransferRole(boundlessMarketAddress);
            console2.log(
                "Granted AUTHORIZED_TRANSFER role to BoundlessMarket on HitPoints collateral token", stakeToken
            );
        }

        vm.stopBroadcast();

        // Update deployment.toml with deployment information
        string memory currentCommit = getCurrentCommit();

        string[] memory args = new string[](8);
        args[0] = "python3";
        args[1] = "contracts/update_deployment_toml.py";
        args[2] = "--boundless-market";
        args[3] = Strings.toHexString(boundlessMarketAddress);
        args[4] = "--boundless-market-impl";
        args[5] = Strings.toHexString(newImplementation);
        args[6] = "--boundless-market-deployment-commit";
        args[7] = currentCommit;

        vm.ffi(args);
        console2.log("Updated BoundlessMarket deployment commit: %s", currentCommit);

        // Also update collateral token if we deployed it
        if (deploymentConfig.collateralToken == address(0)) {
            string[] memory tokenArgs = new string[](4);
            tokenArgs[0] = "python3";
            tokenArgs[1] = "contracts/update_deployment_toml.py";
            tokenArgs[2] = "--collateral-token";
            tokenArgs[3] = Strings.toHexString(stakeToken);
            vm.ffi(tokenArgs);
            console2.log("Updated collateral token address: %s", stakeToken);
        }

        // Check for uncommitted changes warning
        checkUncommittedChangesWarning("Deployment");
    }
}
