// Copyright 2026 Boundless Foundation, Inc.
//
// Use of this source code is governed by the Business Source License
// as found in the LICENSE-BSL file.
// SPDX-License-Identifier: BUSL-1.1

pragma solidity ^0.8.26;

import {Script, console2} from "forge-std/Script.sol";
import {IRiscZeroVerifier} from "risc0/IRiscZeroVerifier.sol";
import {RiscZeroVerifierRouter} from "risc0/RiscZeroVerifierRouter.sol";

import {BoundlessRouter} from "../src/router/BoundlessRouter.sol";
import {R0BoundlessVerifierAdapter} from "../src/router/adapters/R0BoundlessVerifierAdapter.sol";
import {R0BoundlessAssessorAdapter} from "../src/router/adapters/R0BoundlessAssessorAdapter.sol";
import {OnChainAssessor} from "../src/router/adapters/OnChainAssessor.sol";
import {BoundlessScriptBase} from "./BoundlessScript.s.sol";

/// @dev Common base for router-management scripts. Reads the router proxy
///      address and broadcaster key from the environment.
abstract contract RouterManageBase is BoundlessScriptBase {
    bytes4 internal constant R0_VERIFIER_CLASS_ID = bytes4(0xAA000001);
    bytes4 internal constant R0_ASSESSOR_CLASS_ID = bytes4(0xAA000002);

    function _router() internal returns (BoundlessRouter) {
        address routerAddress = vm.envAddress("BOUNDLESS_ROUTER");
        require(routerAddress != address(0), "BOUNDLESS_ROUTER must be set");
        return BoundlessRouter(routerAddress);
    }

    function _broadcast() internal {
        uint256 key = vm.envOr("DEPLOYER_PRIVATE_KEY", uint256(0));
        require(key != 0, "DEPLOYER_PRIVATE_KEY must be set");
        vm.rememberKey(key);
        vm.startBroadcast(key);
    }
}

/// @notice Deploy a `R0BoundlessVerifierAdapter` for one R0 selector and
///         register it under the `R0_VERIFIER` class.
/// @dev    Required env:
///           BOUNDLESS_ROUTER       — router proxy address
///           DEPLOYER_PRIVATE_KEY   — broadcaster (must hold ADMIN_ROLE on
///                                    the router, since `R0_VERIFIER` is
///                                    curated)
///           R0_ROUTER              — upstream `RiscZeroVerifierRouter`
///                                    (used to look up the underlying impl)
///           R0_SELECTOR            — bytes4 selector to register
contract RegisterR0Verifier is RouterManageBase {
    function run() external {
        BoundlessRouter router = _router();
        address r0Router = vm.envAddress("R0_ROUTER");
        bytes4 selector = bytes4(vm.envBytes32("R0_SELECTOR"));

        require(r0Router != address(0), "R0_ROUTER must be set");
        require(selector != bytes4(0), "R0_SELECTOR must be non-zero");

        IRiscZeroVerifier underlying = RiscZeroVerifierRouter(r0Router).getVerifier(selector);
        require(address(underlying) != address(0), "upstream R0 router has no verifier for selector");

        _broadcast();
        R0BoundlessVerifierAdapter adapter = new R0BoundlessVerifierAdapter(underlying);
        router.instantiate(selector, address(adapter), R0_VERIFIER_CLASS_ID, 0);
        vm.stopBroadcast();

        console2.log("Registered R0BoundlessVerifierAdapter at", address(adapter));
        console2.log("Underlying R0 verifier at", address(underlying));
        console2.log("Selector:");
        console2.logBytes4(selector);
    }
}

/// @notice Deploy a `R0BoundlessAssessorAdapter` for one assessor image id and
///         register it under the `R0_ASSESSOR` class at the supplied selector.
/// @dev    Required env:
///           BOUNDLESS_ROUTER       — router proxy address
///           DEPLOYER_PRIVATE_KEY   — broadcaster (must hold ADMIN_ROLE)
///           R0_VERIFIER            — underlying `IRiscZeroVerifier` the
///                                    adapter forwards to (typically the
///                                    `RiscZeroSetVerifier`, since broker
///                                    assessor seals are set-inclusion)
///           ASSESSOR_IMAGE_ID      — guest image id this adapter binds to
///           ASSESSOR_SELECTOR      — bytes4 selector under R0_ASSESSOR.
///                                    Brokers put this in the first 4 bytes
///                                    of the assessor seal.
contract RegisterR0Assessor is RouterManageBase {
    function run() external {
        BoundlessRouter router = _router();
        IRiscZeroVerifier underlying = IRiscZeroVerifier(vm.envAddress("R0_VERIFIER"));
        bytes32 imageId = vm.envBytes32("ASSESSOR_IMAGE_ID");
        bytes4 selector = bytes4(vm.envBytes32("ASSESSOR_SELECTOR"));

        require(address(underlying) != address(0), "R0_VERIFIER must be set");
        require(imageId != bytes32(0), "ASSESSOR_IMAGE_ID must be set");
        require(selector != bytes4(0), "ASSESSOR_SELECTOR must be non-zero");

        _broadcast();
        R0BoundlessAssessorAdapter adapter = new R0BoundlessAssessorAdapter(underlying, imageId);
        router.instantiate(selector, address(adapter), R0_ASSESSOR_CLASS_ID, 0);
        vm.stopBroadcast();

        console2.log("Registered R0BoundlessAssessorAdapter at", address(adapter));
        console2.log("Image id:");
        console2.logBytes32(imageId);
        console2.log("Selector:");
        console2.logBytes4(selector);
    }
}

/// @notice Deploy a native `OnChainAssessor` and register it under the `R0_ASSESSOR`
///         class at the supplied selector. Brokers select it over the R0 STARK assessor
///         by putting this selector in the first 4 bytes of the assessor seal; the broker
///         then signs an EIP-712 `FulfillmentBatchAuth` instead of proving the assessor guest.
/// @dev    Required env:
///           BOUNDLESS_ROUTER          — router proxy address
///           DEPLOYER_PRIVATE_KEY      — broadcaster (must hold ADMIN_ROLE)
///           ONCHAIN_ASSESSOR_SELECTOR — bytes4 selector under R0_ASSESSOR
contract RegisterOnChainAssessor is RouterManageBase {
    function run() external {
        BoundlessRouter router = _router();
        bytes4 selector = bytes4(vm.envBytes32("ONCHAIN_ASSESSOR_SELECTOR"));
        require(selector != bytes4(0), "ONCHAIN_ASSESSOR_SELECTOR must be non-zero");

        _broadcast();
        OnChainAssessor adapter = new OnChainAssessor();
        router.instantiate(selector, address(adapter), R0_ASSESSOR_CLASS_ID, 0);
        vm.stopBroadcast();

        console2.log("Registered OnChainAssessor at", address(adapter));
        console2.log("Selector:");
        console2.logBytes4(selector);
    }
}

/// @notice Tombstone an entry in the router. Once removed, the bytes4 cannot
///         be reused for any class or impl. Use after a broker rollover when
///         a deprecated assessor or verifier is no longer reachable.
/// @dev    Required env:
///           BOUNDLESS_ROUTER       — router proxy address
///           DEPLOYER_PRIVATE_KEY   — broadcaster (must hold ADMIN_ROLE)
///           ENTRY_SELECTOR         — bytes4 to tombstone
contract RemoveEntry is RouterManageBase {
    function run() external {
        BoundlessRouter router = _router();
        bytes4 selector = bytes4(vm.envBytes32("ENTRY_SELECTOR"));
        require(selector != bytes4(0), "ENTRY_SELECTOR must be non-zero");

        _broadcast();
        router.removeEntry(selector);
        vm.stopBroadcast();

        console2.log("Tombstoned entry at selector:");
        console2.logBytes4(selector);
    }
}
