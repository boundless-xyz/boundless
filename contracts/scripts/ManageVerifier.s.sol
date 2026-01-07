// Copyright 2026 Boundless Foundation, Inc.
//
// Use of this source code is governed by the Business Source License
// as found in the LICENSE-BSL file.

pragma solidity ^0.8.9;

import {Script} from "forge-std/Script.sol";
import {console2} from "forge-std/console2.sol";
import {Strings} from "openzeppelin/contracts/utils/Strings.sol";
import {TimelockController} from "openzeppelin/contracts/governance/TimelockController.sol";
import {RiscZeroVerifierRouter} from "../src/verifier/RiscZeroVerifierRouter.sol";
import {VerifierLayeredRouter} from "../src/verifier/VerifierLayeredRouter.sol";
import {RiscZeroVerifierEmergencyStop} from "risc0/RiscZeroVerifierEmergencyStop.sol";
import {IRiscZeroVerifier} from "risc0/IRiscZeroVerifier.sol";
import {IRiscZeroSelectable} from "risc0/IRiscZeroSelectable.sol";
import {Blake3Groth16Verifier} from "../src/blake3-groth16/Blake3Groth16Verifier.sol";
import {ControlID} from "../src/blake3-groth16/ControlID.sol";
import {ConfigLoader, Deployment, DeploymentLib} from "../src/config/VerifierConfig.sol";

// Default salt used with CREATE2 for deterministic deployment addresses.
// NOTE: It kind of spelled risc0 in 1337.
bytes32 constant CREATE2_SALT = hex"1215c0";

/// @notice Compare strings for equality.
function stringEq(string memory a, string memory b) pure returns (bool) {
    return (keccak256(abi.encodePacked((a))) == keccak256(abi.encodePacked((b))));
}

/// @notice Return the role code for the given named role
function timelockControllerRole(TimelockController timelockController, string memory roleStr) view returns (bytes32) {
    if (stringEq(roleStr, "proposer")) {
        return timelockController.PROPOSER_ROLE();
    } else if (stringEq(roleStr, "executor")) {
        return timelockController.EXECUTOR_ROLE();
    } else if (stringEq(roleStr, "canceller")) {
        return timelockController.CANCELLER_ROLE();
    } else {
        revert();
    }
}

/// @notice Base contract for the scripts below, providing common context and functions.
contract RiscZeroManagementScript is Script {
    using DeploymentLib for Deployment;

    Deployment internal deployment;
    TimelockController internal _timelockController;
    VerifierLayeredRouter internal _verifierRouter;
    RiscZeroVerifierRouter internal _parentRouter;
    RiscZeroVerifierEmergencyStop internal _verifierEstop;
    IRiscZeroVerifier internal _verifier;

    function loadConfig() internal {
        string memory configPath =
            vm.envOr("DEPLOYMENT_CONFIG", string.concat(vm.projectRoot(), "/", "contracts/deployment_verifier.toml"));
        console2.log("Loading deployment config from %s", configPath);
        ConfigLoader.loadDeploymentConfig(configPath).copyTo(deployment);

        // Wrap the control addresses with their respective contract implementations.
        // NOTE: These addresses may be zero, so this does not guarantee contracts are deployed.
        _timelockController = TimelockController(payable(deployment.timelockController));
        _verifierRouter = VerifierLayeredRouter(deployment.router);
        _parentRouter = RiscZeroVerifierRouter(deployment.parentRouter);
    }

    modifier withConfig() {
        loadConfig();
        _;
    }

    /// @notice Returns the address of the deployer, set in the DEPLOYER_ADDRESS env var.
    function deployerAddress() internal returns (address) {
        address deployer = vm.envAddress("DEPLOYER_ADDRESS");
        uint256 deployerKey = vm.envOr("DEPLOYER_PRIVATE_KEY", uint256(0));
        if (deployerKey != 0) {
            require(vm.addr(deployerKey) == deployer, "DEPLOYER_ADDRESS and DEPLOYER_PRIVATE_KEY are inconsistent");
            vm.rememberKey(deployerKey);
        }
        return deployer;
    }

    /// @notice Returns the address of the contract admin, set in the ADMIN_ADDRESS env var.
    /// @dev This admin address will be set as the owner of the estop contracts, and the proposer
    ///      of for the timelock controller. Note that it is not the "admin" on the timelock.
    function adminAddress() internal view returns (address) {
        return vm.envOr("ADMIN_ADDRESS", deployment.admin);
    }

    /// @notice Returns the timelock-delay, set in the MIN_DELAY env var.
    function timelockDelay() internal view returns (uint256) {
        return vm.envOr("MIN_DELAY", deployment.timelockDelay);
    }

    /// @notice Determines the contract address of TimelockController from the environment.
    /// @dev Uses the TIMELOCK_CONTROLLER environment variable.
    function timelockController() internal returns (TimelockController) {
        if (address(_timelockController) != address(0)) {
            return _timelockController;
        }
        _timelockController = TimelockController(payable(vm.envAddress("TIMELOCK_CONTROLLER")));
        console2.log("Using TimelockController at address", address(_timelockController));
        return _timelockController;
    }

    /// @notice Determines the contract address of VerifierLayeredRouter from the environment.
    /// @dev Uses the VERIFIER_ROUTER environment variable.
    function verifierRouter() internal returns (VerifierLayeredRouter) {
        if (address(_verifierRouter) != address(0)) {
            return _verifierRouter;
        }
        _verifierRouter = VerifierLayeredRouter(vm.envAddress("VERIFIER_ROUTER"));
        console2.log("Using VerifierLayeredRouter at address", address(_verifierRouter));
        return _verifierRouter;
    }

    function parentRouter() internal returns (RiscZeroVerifierRouter) {
        if (address(_parentRouter) != address(0)) {
            return _parentRouter;
        }
        _parentRouter = RiscZeroVerifierRouter(vm.envAddress("PARENT_VERIFIER_ROUTER"));
        console2.log("Using Parent RiscZeroVerifierRouter at address", address(_parentRouter));
        return _parentRouter;
    }

    /// @notice Determines the contract address of RiscZeroVerifierRouter from the environment.
    /// @dev Uses the VERIFIER_ESTOP environment variable.
    function verifierEstop() internal returns (RiscZeroVerifierEmergencyStop) {
        if (address(_verifierEstop) != address(0)) {
            return _verifierEstop;
        }
        // Use the address set in the VERIFIER_ESTOP environment variable if it is set.
        _verifierEstop = RiscZeroVerifierEmergencyStop(vm.envOr("VERIFIER_ESTOP", address(0)));
        if (address(_verifierEstop) != address(0)) {
            console2.log("Using RiscZeroVerifierEmergencyStop at address", address(_verifierEstop));
            return _verifierEstop;
        }
        bytes4 selector = bytes4(vm.envBytes("VERIFIER_SELECTOR"));
        for (uint256 i = 0; i < deployment.verifiers.length; i++) {
            if (deployment.verifiers[i].selector == selector) {
                _verifierEstop = RiscZeroVerifierEmergencyStop(deployment.verifiers[i].estop);
                break;
            }
        }
        console2.log(
            "Using RiscZeroVerifierEmergencyStop at address %s and selector %x",
            address(_verifierEstop),
            uint256(bytes32(selector))
        );
        return _verifierEstop;
    }

    /// @notice Determines the contract address of IRiscZeroVerifier from the environment.
    /// @dev Uses the VERIFIER_ESTOP environment variable, and gets the proxied verifier.
    function verifier() internal returns (IRiscZeroVerifier) {
        if (address(_verifier) != address(0)) {
            return _verifier;
        }
        _verifier = verifierEstop().verifier();
        console2.log("Using IRiscZeroVerifier at address", address(_verifier));
        return _verifier;
    }

    /// @notice Determines the contract address of IRiscZeroSelectable from the environment.
    /// @dev Uses the VERIFIER_ESTOP environment variable, and gets the proxied selectable.
    function selectable() internal returns (IRiscZeroSelectable) {
        return IRiscZeroSelectable(address(verifier()));
    }

    /// @notice Simulates a call to check if it will succeed, given the current EVM state.
    function simulate(address dest, bytes memory data) internal {
        console2.log("Simulating call to", dest);
        console2.logBytes(data);
        uint256 snapshot = vm.snapshot();
        vm.prank(address(timelockController()));
        (bool success,) = dest.call(data);
        require(success, "simulation of transaction to schedule failed");
        vm.revertTo(snapshot);
        console2.log("Simulation successful");
    }
}

/// @notice Deployment script for the timelocked router.
/// @dev Use the following environment variable to control the deployment:
///     * MIN_DELAY minimum delay in seconds for operations
///     * PROPOSER address of proposer
///     * EXECUTOR address of executor
///
/// See the Foundry documentation for more information about Solidity scripts.
/// https://book.getfoundry.sh/guides/scripting-with-solidity
contract DeployTimelockRouter is RiscZeroManagementScript {
    function run() external withConfig {
        // initial minimum delay in seconds for operations
        uint256 minDelay = timelockDelay();
        console2.log("minDelay:", minDelay);

        // accounts to be granted proposer and canceller roles
        address[] memory proposers = new address[](1);
        proposers[0] = vm.envOr("PROPOSER", adminAddress());
        console2.log("proposers:", proposers[0]);

        // accounts to be granted executor role
        address[] memory executors = new address[](1);
        executors[0] = vm.envOr("EXECUTOR", adminAddress());
        console2.log("executors:", executors[0]);

        // NOTE: This functionality is unused in our process. The admin is not subject to the timelock
        // delay, which is useful e.g. for initial setup, but should not be used in production.
        //
        // optional account to be granted admin role; disable with zero address
        // When the admin is unset, the contract is self-administered.
        //address admin = vm.envOr("ADMIN", address(0));
        //console2.log("admin:", admin);

        // Deploy new contracts
        vm.broadcast(deployerAddress());
        _timelockController = new TimelockController{salt: CREATE2_SALT}(minDelay, proposers, executors, address(0));
        console2.log("Deployed TimelockController to", address(timelockController()));

        vm.broadcast(deployerAddress());
        _verifierRouter = new VerifierLayeredRouter{salt: CREATE2_SALT}(address(timelockController()), parentRouter());
        console2.log("Deployed VerifierLayeredRouter to", address(verifierRouter()));
    }
}

/// @notice Deployment script for the RISC Zero verifier with Emergency Stop mechanism.
/// @dev Use the following environment variable to control the deployment:
///     * CHAIN_KEY key of the target chain
///     * VERIFIER_ESTOP_OWNER owner of the emergency stop contract
///
/// See the Foundry documentation for more information about Solidity scripts.
/// https://book.getfoundry.sh/guides/scripting-with-solidity
contract DeployEstopBlake3Groth16Verifier is RiscZeroManagementScript {
    function run() external withConfig {
        string memory chainKey = vm.envString("CHAIN_KEY");
        console2.log("chainKey:", chainKey);
        address verifierEstopOwner = vm.envOr("VERIFIER_ESTOP_OWNER", adminAddress());
        console2.log("verifierEstopOwner:", verifierEstopOwner);

        // Deploy new contracts
        vm.broadcast(deployerAddress());
        Blake3Groth16Verifier blake3Groth16Verifier =
            new Blake3Groth16Verifier{salt: CREATE2_SALT}(ControlID.CONTROL_ROOT, ControlID.BN254_CONTROL_ID);
        _verifier = blake3Groth16Verifier;

        vm.broadcast(deployerAddress());
        _verifierEstop =
            new RiscZeroVerifierEmergencyStop{salt: CREATE2_SALT}(blake3Groth16Verifier, verifierEstopOwner);

        // Print in TOML format
        console2.log("");
        console2.log("[[chains.%s.verifiers]]", chainKey);
        console2.log("name = \"Blake3Groth16Verifier\"");
        console2.log("version = \"%s\"", blake3Groth16Verifier.VERSION());
        console2.log("selector = \"%s\"", Strings.toHexString(uint256(uint32(blake3Groth16Verifier.SELECTOR())), 4));
        console2.log("verifier = \"%s\"", address(verifier()));
        console2.log("estop = \"%s\"", address(verifierEstop()));
        console2.log("unroutable = true # remove when added to the router");
    }
}

/// @notice Schedule addition of verifier to router.
/// @dev Use the following environment variable to control the deployment:
///     * SCHEDULE_DELAY (optional) minimum delay in seconds for the scheduled action
///     * TIMELOCK_CONTROLLER contract address of TimelockController
///     * VERIFIER_ROUTER contract address of RiscZeroVerifierRouter
///     * VERIFIER_ESTOP contract address of RiscZeroVerifierEmergencyStop
///
/// See the Foundry documentation for more information about Solidity scripts.
/// https://book.getfoundry.sh/guides/scripting-with-solidity
contract ScheduleAddVerifier is RiscZeroManagementScript {
    function run() external withConfig {
        // Check for deployment mode flags
        bool gnosisExecute = vm.envOr("GNOSIS_EXECUTE", false);
        // Schedule the 'addVerifier()' request
        bytes4 selector = selectable().SELECTOR();
        console2.log("Selector: ", Strings.toHexString(uint256(uint32(selector))));

        uint256 scheduleDelay = vm.envOr("SCHEDULE_DELAY", timelockController().getMinDelay());
        console2.log("scheduleDelay: ", scheduleDelay);

        bytes memory data = abi.encodeCall(verifierRouter().addVerifier, (selector, verifierEstop()));
        address dest = address(verifierRouter());
        simulate(dest, data);

        if (gnosisExecute) {
            _printGnosisSafeInfo(address(timelockController()), dest, selector, data, scheduleDelay);
            return;
        }
        vm.broadcast(adminAddress());
        timelockController().schedule(dest, 0, data, 0, 0, scheduleDelay);
    }

    /// @notice Print Gnosis Safe transaction information for manual submissions
    /// @param timelockAddress The timelock controller address (target for Gnosis Safe)
    /// @param dest The destination address for the scheduled operation
    /// @param selector The verifier selector being added
    /// @param data The calldata for the scheduled operation
    /// @param scheduleDelay The minimum delay in seconds for the scheduled action
    function _printGnosisSafeInfo(
        address timelockAddress,
        address dest,
        bytes4 selector,
        bytes memory data,
        uint256 scheduleDelay
    ) internal pure {
        console2.log("================================");
        console2.log("================================");
        console2.log("=== GNOSIS SAFE SCHEDULE ADD VERIFIER INFO ===");
        console2.log("Target Timelock Controller Address (To): ", timelockAddress);
        console2.log("Verifier Router Address (dest): ", dest);
        console2.log("Selector: ", Strings.toHexString(uint256(uint32(selector))));
        console2.log("scheduleDelay: ", scheduleDelay);

        bytes memory callData = abi.encodeWithSignature(
            "schedule(address,uint256,bytes,bytes32,bytes32,uint256)", dest, 0, data, 0, 0, scheduleDelay
        );
        console2.log("Function: schedule(address,uint256,bytes,bytes32,bytes32,uint256)");
        console2.log("Calldata:");
        console2.logBytes(callData);
        console2.log("");
        console2.log("================================");
    }
}

/// @notice Finish addition of verifier to router.
/// @dev Use the following environment variable to control the deployment:
///     * TIMELOCK_CONTROLLER contract address of TimelockController
///     * VERIFIER_ROUTER contract address of RiscZeroVerifierRouter
///     * VERIFIER_ESTOP contract address of RiscZeroVerifierEmergencyStop
///
/// See the Foundry documentation for more information about Solidity scripts.
/// https://book.getfoundry.sh/guides/scripting-with-solidity
contract FinishAddVerifier is RiscZeroManagementScript {
    function run() external withConfig {
        // Check for deployment mode flags
        bool gnosisExecute = vm.envOr("GNOSIS_EXECUTE", false);
        // Execute the 'addVerifier()' request
        bytes4 selector = selectable().SELECTOR();
        console2.log("selector:");
        console2.logBytes4(selector);

        bytes memory data = abi.encodeCall(verifierRouter().addVerifier, (selector, verifierEstop()));

        if (gnosisExecute) {
            _printGnosisSafeInfo(address(timelockController()), address(verifierRouter()), selector, data);
            return;
        }

        vm.broadcast(adminAddress());
        timelockController().execute(address(verifierRouter()), 0, data, 0, 0);
    }

    /// @notice Print Gnosis Safe transaction information for manual submissions
    /// @param timelockAddress The timelock controller address (target for Gnosis Safe)
    /// @param dest The destination address for the scheduled operation
    /// @param selector The verifier selector being added
    /// @param data The calldata for the scheduled operation
    function _printGnosisSafeInfo(address timelockAddress, address dest, bytes4 selector, bytes memory data)
        internal
        pure
    {
        console2.log("================================");
        console2.log("================================");
        console2.log("=== GNOSIS SAFE EXECUTE ADD VERIFIER INFO ===");
        console2.log("Target Timelock Controller Address (To): ", timelockAddress);
        console2.log("Verifier Router Address (dest): ", dest);
        console2.log("Selector: ", Strings.toHexString(uint256(uint32(selector))));

        bytes memory callData =
            abi.encodeWithSignature("execute(address,uint256,bytes,bytes32,bytes32)", dest, 0, data, 0, 0);
        console2.log("Function: execute(address,uint256,bytes,bytes32,bytes32)");
        console2.log("Calldata:");
        console2.logBytes(callData);
        console2.log("");
        console2.log("================================");
    }
}

/// @notice Schedule removal of a verifier from the router.
/// @dev Use the following environment variable to control the deployment:
///     * VERIFIER_SELECTOR the selector associated with this verifier
///     * SCHEDULE_DELAY (optional) minimum delay in seconds for the scheduled action
///     * TIMELOCK_CONTROLLER contract address of TimelockController
///     * VERIFIER_ROUTER contract address of RiscZeroVerifierRouter
///
/// See the Foundry documentation for more information about Solidity scripts.
/// https://book.getfoundry.sh/guides/scripting-with-solidity
contract ScheduleRemoveVerifier is RiscZeroManagementScript {
    function run() external withConfig {
        // Check for deployment mode flags
        bool gnosisExecute = vm.envOr("GNOSIS_EXECUTE", false);
        bytes4 selector = bytes4(vm.envBytes("VERIFIER_SELECTOR"));
        console2.log("selector:");
        console2.logBytes4(selector);

        // Schedule the 'removeVerifier()' request
        uint256 scheduleDelay = vm.envOr("SCHEDULE_DELAY", timelockController().getMinDelay());
        console2.log("scheduleDelay:", scheduleDelay);

        bytes memory data = abi.encodeCall(verifierRouter().removeVerifier, selector);
        address dest = address(verifierRouter());
        simulate(dest, data);

        if (gnosisExecute) {
            _printGnosisSafeInfo(address(timelockController()), dest, selector, data, scheduleDelay);
            return;
        }

        vm.broadcast(adminAddress());
        timelockController().schedule(dest, 0, data, 0, 0, scheduleDelay);
    }

    /// @notice Print Gnosis Safe transaction information for manual submissions
    /// @param timelockAddress The timelock controller address (target for Gnosis Safe)
    /// @param dest The destination address for the scheduled operation
    /// @param selector The verifier selector being removed
    /// @param data The calldata for the scheduled operation
    /// @param scheduleDelay The minimum delay in seconds for the scheduled action
    function _printGnosisSafeInfo(
        address timelockAddress,
        address dest,
        bytes4 selector,
        bytes memory data,
        uint256 scheduleDelay
    ) internal pure {
        console2.log("================================");
        console2.log("================================");
        console2.log("=== GNOSIS SAFE SCHEDULE REMOVE VERIFIER INFO ===");
        console2.log("Target Timelock Controller Address (To): ", timelockAddress);
        console2.log("Verifier Router Address (dest): ", dest);
        console2.log("Selector: ", Strings.toHexString(uint256(uint32(selector))));
        console2.log("scheduleDelay: ", scheduleDelay);

        bytes memory callData = abi.encodeWithSignature(
            "schedule(address,uint256,bytes,bytes32,bytes32,uint256)", dest, 0, data, 0, 0, scheduleDelay
        );
        console2.log("Function: schedule(address,uint256,bytes,bytes32,bytes32,uint256)");
        console2.log("Calldata:");
        console2.logBytes(callData);
        console2.log("");
        console2.log("================================");
    }
}

/// @notice Finish removal of a verifier from the router.
/// @dev Use the following environment variable to control the deployment:
///     * VERIFIER_SELECTOR the selector associated with this verifier
///     * TIMELOCK_CONTROLLER contract address of TimelockController
///     * VERIFIER_ROUTER contract address of RiscZeroVerifierRouter
///
/// See the Foundry documentation for more information about Solidity scripts.
/// https://book.getfoundry.sh/guides/scripting-with-solidity
contract FinishRemoveVerifier is RiscZeroManagementScript {
    function run() external withConfig {
        // Check for deployment mode flags
        bool gnosisExecute = vm.envOr("GNOSIS_EXECUTE", false);
        bytes4 selector = bytes4(vm.envBytes("VERIFIER_SELECTOR"));
        console2.log("selector:");
        console2.logBytes4(selector);

        // Execute the 'removeVerifier()' request
        bytes memory data = abi.encodeCall(verifierRouter().removeVerifier, selector);

        if (gnosisExecute) {
            _printGnosisSafeInfo(address(timelockController()), address(verifierRouter()), selector, data);
            return;
        }

        vm.broadcast(adminAddress());
        timelockController().execute(address(verifierRouter()), 0, data, 0, 0);
    }

    /// @notice Print Gnosis Safe transaction information for manual submissions
    /// @param timelockAddress The timelock controller address (target for Gnosis Safe)
    /// @param dest The destination address for the scheduled operation
    /// @param selector The verifier selector being removed
    /// @param data The calldata for the scheduled operation
    function _printGnosisSafeInfo(address timelockAddress, address dest, bytes4 selector, bytes memory data)
        internal
        pure
    {
        console2.log("================================");
        console2.log("================================");
        console2.log("=== GNOSIS SAFE EXECUTE REMOVE VERIFIER INFO ===");
        console2.log("Target Timelock Controller Address (To): ", timelockAddress);
        console2.log("Verifier Router Address (dest): ", dest);
        console2.log("Selector: ", Strings.toHexString(uint256(uint32(selector))));

        bytes memory callData =
            abi.encodeWithSignature("execute(address,uint256,bytes,bytes32,bytes32)", dest, 0, data, 0, 0);
        console2.log("Function: execute(address,uint256,bytes,bytes32,bytes32)");
        console2.log("Calldata:");
        console2.logBytes(callData);
        console2.log("");
        console2.log("================================");
    }
}

/// @notice Schedule an update of the minimum timelock delay.
/// @dev Use the following environment variable to control the deployment:
///     * MIN_DELAY minimum delay in seconds for operations
///     * SCHEDULE_DELAY (optional) minimum delay in seconds for the scheduled action
///     * TIMELOCK_CONTROLLER contract address of TimelockController
///
/// See the Foundry documentation for more information about Solidity scripts.
/// https://book.getfoundry.sh/guides/scripting-with-solidity
contract ScheduleUpdateDelay is RiscZeroManagementScript {
    function run() external withConfig {
        // Check for deployment mode flags
        bool gnosisExecute = vm.envOr("GNOSIS_EXECUTE", false);
        uint256 minDelay = vm.envUint("MIN_DELAY");
        console2.log("minDelay:", minDelay);

        // Schedule the 'updateDelay()' request
        uint256 scheduleDelay = vm.envOr("SCHEDULE_DELAY", timelockController().getMinDelay());
        console2.log("scheduleDelay:", scheduleDelay);

        bytes memory data = abi.encodeCall(timelockController().updateDelay, minDelay);
        address dest = address(timelockController());
        simulate(dest, data);

        if (gnosisExecute) {
            _printGnosisSafeInfo(address(timelockController()), minDelay, data, scheduleDelay);
            return;
        }

        vm.broadcast(adminAddress());
        timelockController().schedule(dest, 0, data, 0, 0, scheduleDelay);
    }

    /// @notice Print Gnosis Safe transaction information for manual submissions
    /// @param timelockAddress The timelock controller address (target for Gnosis Safe)
    /// @param minDelay The new minimum delay
    /// @param data The calldata for the scheduled operation
    /// @param scheduleDelay The minimum delay in seconds for the scheduled action
    function _printGnosisSafeInfo(address timelockAddress, uint256 minDelay, bytes memory data, uint256 scheduleDelay)
        internal
        pure
    {
        console2.log("================================");
        console2.log("================================");
        console2.log("=== GNOSIS SAFE SCHEDULE MIN DELAY INFO ===");
        console2.log("Target Timelock Controller Address (To): ", timelockAddress);
        console2.log("New min delay: ", minDelay);
        console2.log("scheduleDelay: ", scheduleDelay);

        bytes memory callData = abi.encodeWithSignature(
            "schedule(address,uint256,bytes,bytes32,bytes32,uint256)", timelockAddress, 0, data, 0, 0, scheduleDelay
        );
        console2.log("Function: schedule(address,uint256,bytes,bytes32,bytes32,uint256)");
        console2.log("Calldata:");
        console2.logBytes(callData);
        console2.log("");
        console2.log("================================");
    }
}

/// @notice Finish an update of the minimum timelock delay.
/// @dev Use the following environment variable to control the deployment:
///     * MIN_DELAY minimum delay in seconds for operations
///     * TIMELOCK_CONTROLLER contract address of TimelockController
///
/// See the Foundry documentation for more information about Solidity scripts.
/// https://book.getfoundry.sh/guides/scripting-with-solidity
contract FinishUpdateDelay is RiscZeroManagementScript {
    function run() external withConfig {
        // Check for deployment mode flags
        bool gnosisExecute = vm.envOr("GNOSIS_EXECUTE", false);
        uint256 minDelay = vm.envUint("MIN_DELAY");
        console2.log("minDelay:", minDelay);

        // Execute the 'updateDelay()' request
        bytes memory data = abi.encodeCall(timelockController().updateDelay, minDelay);

        if (gnosisExecute) {
            _printGnosisSafeInfo(address(timelockController()), minDelay, data);
            return;
        }

        vm.broadcast(adminAddress());
        timelockController().execute(address(timelockController()), 0, data, 0, 0);
    }

    /// @notice Print Gnosis Safe transaction information for manual submissions
    /// @param timelockAddress The timelock controller address (target for Gnosis Safe)
    /// @param minDelay The new minimum delay
    /// @param data The calldata for the scheduled operation
    function _printGnosisSafeInfo(address timelockAddress, uint256 minDelay, bytes memory data) internal pure {
        console2.log("================================");
        console2.log("================================");
        console2.log("=== GNOSIS SAFE EXECUTE MIN DELAY INFO ===");
        console2.log("Target Timelock Controller Address (To): ", timelockAddress);
        console2.log("New min delay: ", minDelay);

        bytes memory callData =
            abi.encodeWithSignature("execute(address,uint256,bytes,bytes32,bytes32)", timelockAddress, 0, data, 0, 0);
        console2.log("Function: execute(address,uint256,bytes,bytes32,bytes32)");
        console2.log("Calldata:");
        console2.logBytes(callData);
        console2.log("");
        console2.log("================================");
    }
}

// TODO: Add this command to the README.md
/// @notice Cancel a pending operation on the timelock controller
/// @dev Use the following environment variable to control the script:
///     * TIMELOCK_CONTROLLER contract address of TimelockController
///     * OPERATION_ID identifier for the operation to cancel
///
/// See the Foundry documentation for more information about Solidity scripts.
/// https://book.getfoundry.sh/guides/scripting-with-solidity
contract CancelOperation is RiscZeroManagementScript {
    function run() external withConfig {
        // Check for deployment mode flags
        bool gnosisExecute = vm.envOr("GNOSIS_EXECUTE", false);
        bytes32 operationId = vm.envBytes32("OPERATION_ID");
        console2.log("operationId:", uint256(operationId));

        if (gnosisExecute) {
            _printGnosisSafeInfo(address(timelockController()), operationId);
            return;
        }

        // Execute the 'cancel()' request
        vm.broadcast(adminAddress());
        timelockController().cancel(operationId);
    }

    /// @notice Print Gnosis Safe transaction information for manual submissions
    function _printGnosisSafeInfo(address timelockAddress, bytes32 operationId) internal pure {
        console2.log("================================");
        console2.log("================================");
        console2.log("=== GNOSIS SAFE CANCEL OPERATION INFO ===");
        console2.log("Target Timelock Controller Address (To): ", timelockAddress);
        console2.log("Operation ID: ", uint256(operationId));

        bytes memory callData = abi.encodeWithSignature("cancel(bytes32)", operationId);
        console2.log("Function: cancel(bytes32)");
        console2.log("Calldata:");
        console2.logBytes(callData);
        console2.log("");
        console2.log("================================");
    }
}

/// @notice Schedule grant role.
/// @dev Use the following environment variable to control the deployment:
///     * ROLE the role to be granted
///     * ACCOUNT the account to be granted the role
///     * SCHEDULE_DELAY (optional) minimum delay in seconds for the scheduled action
///     * TIMELOCK_CONTROLLER contract address of TimelockController
///
/// See the Foundry documentation for more information about Solidity scripts.
/// https://book.getfoundry.sh/guides/scripting-with-solidity
contract ScheduleGrantRole is RiscZeroManagementScript {
    function run() external withConfig {
        // Check for deployment mode flags
        bool gnosisExecute = vm.envOr("GNOSIS_EXECUTE", false);
        string memory roleStr = vm.envString("ROLE");
        console2.log("roleStr:", roleStr);

        address account = vm.envAddress("ACCOUNT");
        console2.log("account:", account);

        // Schedule the 'grantRole()' request
        bytes32 role = timelockControllerRole(timelockController(), roleStr);
        console2.log("role: ");
        console2.logBytes32(role);

        uint256 scheduleDelay = vm.envOr("SCHEDULE_DELAY", timelockController().getMinDelay());
        console2.log("scheduleDelay:", scheduleDelay);

        bytes memory data = abi.encodeCall(timelockController().grantRole, (role, account));
        address dest = address(timelockController());
        simulate(dest, data);

        if (gnosisExecute) {
            _printGnosisSafeInfo(address(timelockController()), role, account, data, scheduleDelay);
            return;
        }

        vm.broadcast(adminAddress());
        timelockController().schedule(dest, 0, data, 0, 0, scheduleDelay);
    }

    /// @notice Print Gnosis Safe transaction information for manual submissions
    function _printGnosisSafeInfo(
        address timelockAddress,
        bytes32 role,
        address account,
        bytes memory data,
        uint256 scheduleDelay
    ) internal pure {
        console2.log("================================");
        console2.log("================================");
        console2.log("=== GNOSIS SAFE SCHEDULE GRANT ROLE INFO ===");
        console2.log("Target Timelock Controller Address (To): ", timelockAddress);
        console2.log("Role: ", uint256(role));
        console2.log("Account: ", account);
        console2.log("scheduleDelay: ", scheduleDelay);

        bytes memory callData = abi.encodeWithSignature(
            "schedule(address,uint256,bytes,bytes32,bytes32,uint256)", timelockAddress, 0, data, 0, 0, scheduleDelay
        );
        console2.log("Function: schedule(address,uint256,bytes,bytes32,bytes32,uint256)");
        console2.log("Calldata:");
        console2.logBytes(callData);
        console2.log("");
        console2.log("================================");
    }
}

/// @notice Finish grant role.
/// @dev Use the following environment variable to control the deployment:
///     * ROLE the role to be granted
///     * ACCOUNT the account to be granted the role
///     * TIMELOCK_CONTROLLER contract address of TimelockController
///
/// See the Foundry documentation for more information about Solidity scripts.
/// https://book.getfoundry.sh/guides/scripting-with-solidity
contract FinishGrantRole is RiscZeroManagementScript {
    function run() external withConfig {
        // Check for deployment mode flags
        bool gnosisExecute = vm.envOr("GNOSIS_EXECUTE", false);
        string memory roleStr = vm.envString("ROLE");
        console2.log("roleStr:", roleStr);

        address account = vm.envAddress("ACCOUNT");
        console2.log("account:", account);

        // Execute the 'grantRole()' request
        bytes32 role = timelockControllerRole(timelockController(), roleStr);
        console2.log("role: ");
        console2.logBytes32(role);

        bytes memory data = abi.encodeCall(timelockController().grantRole, (role, account));

        if (gnosisExecute) {
            _printGnosisSafeInfo(address(timelockController()), role, account, data);
            return;
        }

        vm.broadcast(adminAddress());
        timelockController().execute(address(timelockController()), 0, data, 0, 0);
    }

    /// @notice Print Gnosis Safe transaction information for manual submissions
    function _printGnosisSafeInfo(address timelockAddress, bytes32 role, address account, bytes memory data)
        internal
        pure
    {
        console2.log("================================");
        console2.log("================================");
        console2.log("=== GNOSIS SAFE EXECUTE GRANT ROLE INFO ===");
        console2.log("Target Timelock Controller Address (To): ", timelockAddress);
        console2.log("Role: ", uint256(role));
        console2.log("Account: ", account);

        bytes memory callData =
            abi.encodeWithSignature("execute(address,uint256,bytes,bytes32,bytes32)", timelockAddress, 0, data, 0, 0);
        console2.log("Function: execute(address,uint256,bytes,bytes32,bytes32)");
        console2.log("Calldata:");
        console2.logBytes(callData);
        console2.log("");
        console2.log("================================");
    }
}

/// @notice Schedule revoke role.
/// @dev Use the following environment variable to control the deployment:
///     * ROLE the role to be revoked
///     * ACCOUNT the account to be revoked of the role
///     * SCHEDULE_DELAY (optional) minimum delay in seconds for the scheduled action
///     * TIMELOCK_CONTROLLER contract address of TimelockController
///
/// See the Foundry documentation for more information about Solidity scripts.
/// https://book.getfoundry.sh/guides/scripting-with-solidity
contract ScheduleRevokeRole is RiscZeroManagementScript {
    function run() external withConfig {
        // Check for deployment mode flags
        bool gnosisExecute = vm.envOr("GNOSIS_EXECUTE", false);
        string memory roleStr = vm.envString("ROLE");
        console2.log("roleStr:", roleStr);

        address account = vm.envAddress("ACCOUNT");
        console2.log("account:", account);

        // Schedule the 'grantRole()' request
        bytes32 role = timelockControllerRole(timelockController(), roleStr);
        console2.log("role: ");
        console2.logBytes32(role);

        uint256 scheduleDelay = vm.envOr("SCHEDULE_DELAY", timelockController().getMinDelay());
        console2.log("scheduleDelay:", scheduleDelay);

        bytes memory data = abi.encodeCall(timelockController().revokeRole, (role, account));
        address dest = address(timelockController());
        simulate(dest, data);

        if (gnosisExecute) {
            _printGnosisSafeInfo(address(timelockController()), role, account, data, scheduleDelay);
            return;
        }

        vm.broadcast(adminAddress());
        timelockController().schedule(dest, 0, data, 0, 0, scheduleDelay);
    }

    /// @notice Print Gnosis Safe transaction information for manual submissions
    function _printGnosisSafeInfo(
        address timelockAddress,
        bytes32 role,
        address account,
        bytes memory data,
        uint256 scheduleDelay
    ) internal pure {
        console2.log("================================");
        console2.log("================================");
        console2.log("=== GNOSIS SAFE SCHEDULE REVOKE ROLE INFO ===");
        console2.log("Target Timelock Controller Address (To): ", timelockAddress);
        console2.log("Role: ", uint256(role));
        console2.log("Account: ", account);
        console2.log("scheduleDelay: ", scheduleDelay);

        bytes memory callData = abi.encodeWithSignature(
            "schedule(address,uint256,bytes,bytes32,bytes32,uint256)", timelockAddress, 0, data, 0, 0, scheduleDelay
        );
        console2.log("Function: schedule(address,uint256,bytes,bytes32,bytes32,uint256)");
        console2.log("Calldata:");
        console2.logBytes(callData);
        console2.log("");
        console2.log("================================");
    }
}

/// @notice Finish revoke role.
/// @dev Use the following environment variable to control the deployment:
///     * ROLE the role to be revoked
///     * ACCOUNT the account to be revoked of the role
///     * TIMELOCK_CONTROLLER contract address of TimelockController
///
/// See the Foundry documentation for more information about Solidity scripts.
/// https://book.getfoundry.sh/guides/scripting-with-solidity
contract FinishRevokeRole is RiscZeroManagementScript {
    function run() external withConfig {
        // Check for deployment mode flags
        bool gnosisExecute = vm.envOr("GNOSIS_EXECUTE", false);
        string memory roleStr = vm.envString("ROLE");
        console2.log("roleStr:", roleStr);

        address account = vm.envAddress("ACCOUNT");
        console2.log("account:", account);

        // Execute the 'grantRole()' request
        bytes32 role = timelockControllerRole(timelockController(), roleStr);
        console2.log("role: ");
        console2.logBytes32(role);

        bytes memory data = abi.encodeCall(timelockController().revokeRole, (role, account));

        if (gnosisExecute) {
            _printGnosisSafeInfo(address(timelockController()), role, account, data);
            return;
        }

        vm.broadcast(adminAddress());
        timelockController().execute(address(timelockController()), 0, data, 0, 0);
    }

    /// @notice Print Gnosis Safe transaction information for manual submissions
    function _printGnosisSafeInfo(address timelockAddress, bytes32 role, address account, bytes memory data)
        internal
        pure
    {
        console2.log("================================");
        console2.log("================================");
        console2.log("=== GNOSIS SAFE EXECUTE REVOKE ROLE INFO ===");
        console2.log("Target Timelock Controller Address (To): ", timelockAddress);
        console2.log("Role: ", uint256(role));
        console2.log("Account: ", account);

        bytes memory callData =
            abi.encodeWithSignature("execute(address,uint256,bytes,bytes32,bytes32)", timelockAddress, 0, data, 0, 0);
        console2.log("Function: execute(address,uint256,bytes,bytes32,bytes32)");
        console2.log("Calldata:");
        console2.logBytes(callData);
        console2.log("");
        console2.log("================================");
    }
}

/// @notice Renounce role.
/// @dev Use the following environment variable to control the deployment:
///     * RENOUNCE_ADDRESS the address to send the renounce transaction
///     * RENOUNCE_ROLE the role to be renounced
///     * TIMELOCK_CONTROLLER contract address of TimelockController
///
/// See the Foundry documentation for more information about Solidity scripts.
/// https://book.getfoundry.sh/guides/scripting-with-solidity
contract RenounceRole is RiscZeroManagementScript {
    function run() external withConfig {
        address renouncer = vm.envAddress("RENOUNCE_ADDRESS");
        string memory roleStr = vm.envString("RENOUNCE_ROLE");
        console2.log("renouncer:", renouncer);
        console2.log("roleStr:", roleStr);

        console2.log("msg.sender:", msg.sender);

        // Renounce the role
        bytes32 role = timelockControllerRole(timelockController(), roleStr);
        console2.log("role: ");
        console2.logBytes32(role);

        vm.broadcast(renouncer);
        timelockController().renounceRole(role, msg.sender);
    }
}

/// @notice Activate an Emergency Stop mechanism.
/// @dev Use the following environment variable to control the deployment:
///     * VERIFIER_ESTOP contract address of RiscZeroVerifierEmergencyStop
///
/// See the Foundry documentation for more information about Solidity scripts.
/// https://book.getfoundry.sh/guides/scripting-with-solidity
contract ActivateEstop is RiscZeroManagementScript {
    function run() external withConfig {
        // Check for deployment mode flags
        bool gnosisExecute = vm.envOr("GNOSIS_EXECUTE", false);
        // Locate contracts
        console2.log("Using RiscZeroVerifierEmergencyStop at address", address(verifierEstop()));

        if (gnosisExecute) {
            _printGnosisSafeInfo(address(verifierEstop()));
            return;
        }
        // Activate the emergency stop
        vm.broadcast(adminAddress());
        verifierEstop().estop();
        require(verifierEstop().paused(), "verifier is not stopped after calling estop");
    }

    // @notice Print Gnosis Safe transaction information for manual submissions
    function _printGnosisSafeInfo(address estopAddress) internal pure {
        console2.log("================================");
        console2.log("================================");
        console2.log("=== GNOSIS SAFE ACTIVATE EMERGENCY STOP INFO ===");
        console2.log("RiscZeroVerifierEmergencyStop Address (To): ", estopAddress);
        bytes memory callData = abi.encodeWithSignature("estop()");
        console2.log("Function: estop()");
        console2.log("Calldata:");
        console2.logBytes(callData);
        console2.log("");
        console2.log("================================");
    }
}
