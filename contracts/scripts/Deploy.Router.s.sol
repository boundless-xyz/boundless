// Copyright 2026 Boundless Foundation, Inc.
//
// Use of this source code is governed by the Business Source License
// as found in the LICENSE-BSL file.
// SPDX-License-Identifier: BUSL-1.1

pragma solidity ^0.8.26;

import {console2} from "forge-std/Script.sol";
import {ERC1967Proxy} from "@openzeppelin/contracts/proxy/ERC1967/ERC1967Proxy.sol";

import {BoundlessRouter} from "../src/router/BoundlessRouter.sol";
import {BoundlessScriptBase} from "./BoundlessScript.s.sol";

/// @notice Deploy the `BoundlessRouter` UUPS proxy. Class and entry registration is
///         handled by `Manage.Router.s.sol:BootstrapRouter` — run it next.
/// @dev    Required env vars:
///           DEPLOYER_PRIVATE_KEY  — broadcaster
///           ROUTER_ADMIN          — admin address granted ADMIN_ROLE on the
///                                   router (governs class / entry mutations
///                                   and UUPS upgrades). Bring-up typically uses
///                                   the deployer EOA and hands the role to the
///                                   Safe/timelock afterwards via
///                                   `TransferRouterAdmin`.
contract DeployRouter is BoundlessScriptBase {
    function run() external {
        uint256 deployerKey = vm.envOr("DEPLOYER_PRIVATE_KEY", uint256(0));
        require(deployerKey != 0, "No deployer key provided. Set DEPLOYER_PRIVATE_KEY.");
        vm.rememberKey(deployerKey);

        address admin = vm.envAddress("ROUTER_ADMIN");
        console2.log("BoundlessRouter admin:", admin);

        vm.startBroadcast(deployerKey);
        BoundlessRouter implementation = new BoundlessRouter();
        address proxy =
            address(new ERC1967Proxy(address(implementation), abi.encodeCall(BoundlessRouter.initialize, (admin))));
        vm.stopBroadcast();

        console2.log("Deployed BoundlessRouter implementation at", address(implementation));
        console2.log("Deployed BoundlessRouter (proxy) at", proxy);
        console2.log("Run Manage.Router.s.sol:BootstrapRouter to register classes and entries.");
    }
}
