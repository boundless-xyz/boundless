// Copyright 2026 Boundless Foundation, Inc.
//
// Use of this source code is governed by the Business Source License
// as found in the LICENSE-BSL file.
// SPDX-License-Identifier: BUSL-1.1

pragma solidity ^0.8.26;

import {console2} from "forge-std/Script.sol";
import {ERC1967Proxy} from "@openzeppelin/contracts/proxy/ERC1967/ERC1967Proxy.sol";
import {Strings} from "@openzeppelin/contracts/utils/Strings.sol";

import {BoundlessRouter} from "../src/router/BoundlessRouter.sol";
import {BoundlessScriptBase} from "./BoundlessScript.s.sol";
import {ConfigLoader, DeploymentConfig} from "./Config.s.sol";

/// @notice Deploy the `BoundlessRouter` UUPS proxy. Class and entry registration is
///         handled by `Manage.Router.s.sol:BootstrapRouter` — run it next.
/// @dev    Admin comes from the `CHAIN_KEY` section's `admin` in `deployment.toml`,
///         overridable via ROUTER_ADMIN. The admin holds ADMIN_ROLE on the router
///         (governs class / entry mutations and UUPS upgrades); bring-up typically
///         uses the deployer EOA and hands the role to the Safe/timelock afterwards
///         via `TransferRouterAdmin`. DEPLOYER_PRIVATE_KEY is the broadcaster.
contract DeployRouter is BoundlessScriptBase {
    function run() external {
        uint256 deployerKey = vm.envOr("DEPLOYER_PRIVATE_KEY", uint256(0));
        require(deployerKey != 0, "No deployer key provided. Set DEPLOYER_PRIVATE_KEY.");
        vm.rememberKey(deployerKey);

        DeploymentConfig memory deploymentConfig =
            ConfigLoader.loadDeploymentConfig(string.concat(vm.projectRoot(), "/", CONFIG));
        address admin = vm.envOr("ROUTER_ADMIN", deploymentConfig.admin);
        require(admin != address(0), "set admin in deployment.toml or ROUTER_ADMIN");
        console2.log("BoundlessRouter admin:", admin);

        vm.startBroadcast(deployerKey);
        BoundlessRouter implementation = new BoundlessRouter();
        address proxy =
            address(new ERC1967Proxy(address(implementation), abi.encodeCall(BoundlessRouter.initialize, (admin))));
        vm.stopBroadcast();

        console2.log("Deployed BoundlessRouter implementation at", address(implementation));
        console2.log("Deployed BoundlessRouter (proxy) at", proxy);

        // Record the proxy in deployment.toml (CHAIN_KEY selects the section).
        string[] memory args = new string[](4);
        args[0] = "python3";
        args[1] = "contracts/update_deployment_toml.py";
        args[2] = "--boundless-router";
        args[3] = Strings.toHexString(proxy);
        vm.ffi(args);

        console2.log("Run Manage.Router.s.sol:BootstrapRouter to register classes and entries.");
    }
}
