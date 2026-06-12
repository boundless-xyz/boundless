// Copyright 2026 Boundless Foundation, Inc.
//
// Use of this source code is governed by the Business Source License
// as found in the LICENSE-BSL file.
// SPDX-License-Identifier: BUSL-1.1

pragma solidity ^0.8.26;

import {console2} from "forge-std/Script.sol";
import {Strings} from "openzeppelin/contracts/utils/Strings.sol";
import {ERC1967Proxy} from "@openzeppelin/contracts/proxy/ERC1967/ERC1967Proxy.sol";

import {BoundlessRouter} from "../src/router/BoundlessRouter.sol";
import {IBoundlessVerifier} from "../src/router/interfaces/IBoundlessVerifier.sol";
import {IBoundlessAssessor} from "../src/router/interfaces/IBoundlessAssessor.sol";
import {BoundlessScriptBase} from "./BoundlessScript.s.sol";

/// @notice Deploy the `BoundlessRouter` UUPS proxy and register the curated
///         R0 classes (`R0_VERIFIER` as the chain default, `R0_ASSESSOR` as
///         its required assessor class).
/// @dev    Bootstrap-only. Adapter / entry registration is handled by
///         `Manage.Router.s.sol`. Re-running this script against an already-
///         deployed router is unsafe and will revert at the `addClass` step.
///
///         Required env vars:
///           DEPLOYER_PRIVATE_KEY  — broadcaster
///           ROUTER_ADMIN          — admin address granted ADMIN_ROLE on the
///                                   router (governs class / entry mutations
///                                   and UUPS upgrades). Typically the same
///                                   timelock controller as the rest of the
///                                   Boundless deployment.
contract DeployRouter is BoundlessScriptBase {
    /// @notice Curated class id for the R0 STARK verifier seam.
    bytes4 internal constant R0_VERIFIER_CLASS_ID = bytes4(0xAA000001);
    /// @notice Curated class id for the R0 STARK assessor seam.
    bytes4 internal constant R0_ASSESSOR_CLASS_ID = bytes4(0xAA000002);

    function run() external {
        uint256 deployerKey = vm.envOr("DEPLOYER_PRIVATE_KEY", uint256(0));
        require(deployerKey != 0, "No deployer key provided. Set DEPLOYER_PRIVATE_KEY.");
        vm.rememberKey(deployerKey);

        address admin = vm.envAddress("ROUTER_ADMIN");
        console2.log("BoundlessRouter admin:", admin);

        vm.startBroadcast(deployerKey);

        // Deploy the UUPS proxy.
        BoundlessRouter implementation = new BoundlessRouter();
        address proxy =
            address(new ERC1967Proxy(address(implementation), abi.encodeCall(BoundlessRouter.initialize, (admin))));
        BoundlessRouter router = BoundlessRouter(proxy);
        console2.log("Deployed BoundlessRouter implementation at", address(implementation));
        console2.log("Deployed BoundlessRouter (proxy) at", proxy);

        // Register R0_ASSESSOR first; the verifier class references it via
        // `requiredAssessorClass`, so it must already exist when we add
        // R0_VERIFIER.
        BoundlessRouter.ClassMetadata memory assessorMeta = BoundlessRouter.ClassMetadata({
            interfaceTag: type(IBoundlessAssessor).interfaceId,
            permissionlessInstantiate: false,
            isDefault: false,
            requiredAssessorClass: bytes4(0),
            schemaArtifact: bytes32(0),
            schemaArtifactUrl: "",
            defaultGasLimit: 200_000,
            label: "R0 STARK assessor"
        });
        router.addClass(R0_ASSESSOR_CLASS_ID, assessorMeta);
        console2.log("Registered R0_ASSESSOR class at id");
        console2.logBytes4(R0_ASSESSOR_CLASS_ID);

        // R0_VERIFIER as the chain default class. Default-class designation is
        // exclusive at the router level; it lives here because today's
        // requestors that sign `0x00000000` (chain default) expect to dispatch
        // through the R0 STARK verifier path.
        BoundlessRouter.ClassMetadata memory verifierMeta = BoundlessRouter.ClassMetadata({
            interfaceTag: type(IBoundlessVerifier).interfaceId,
            permissionlessInstantiate: false,
            isDefault: true,
            requiredAssessorClass: R0_ASSESSOR_CLASS_ID,
            schemaArtifact: bytes32(0),
            schemaArtifactUrl: "",
            // Must cover the most expensive curated verifier plus adapter overhead: a real
            // Groth16 verification costs ~250k gas (blake3-groth16 slightly more). The cap bounds
            // what a runaway adapter can burn per fill, not the expected cost — dev-mode mock
            // verifiers masked this until real-proving runs hit VerifierFailed at 50k.
            defaultGasLimit: 500_000,
            label: "R0 STARK verifier"
        });
        router.addClass(R0_VERIFIER_CLASS_ID, verifierMeta);
        console2.log("Registered R0_VERIFIER class (chain default) at id");
        console2.logBytes4(R0_VERIFIER_CLASS_ID);

        vm.stopBroadcast();

        console2.log("BoundlessRouter ready at %s. Run Manage.Router.s.sol to register adapter entries.", proxy);
    }
}
