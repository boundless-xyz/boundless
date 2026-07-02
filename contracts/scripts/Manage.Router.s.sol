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

import {BoundlessRouter} from "../src/router/BoundlessRouter.sol";
import {R0BoundlessVerifierAdapter} from "../src/router/adapters/R0BoundlessVerifierAdapter.sol";
import {R0BoundlessAssessorAdapter} from "../src/router/adapters/R0BoundlessAssessorAdapter.sol";
import {OnChainAssessor} from "../src/router/adapters/OnChainAssessor.sol";
import {BoundlessScriptBase} from "./BoundlessScript.s.sol";
import {ConfigLoader, DeploymentConfig} from "./Config.s.sol";
import {RouterConfig} from "./RouterConfig.s.sol";

/// @dev Common base for router-management scripts. Inputs resolve from the `CHAIN_KEY`
///      section of `deployment.toml`, each overridable by an env var of the listed name.
abstract contract RouterManageBase is BoundlessScriptBase {
    function _config() internal view returns (DeploymentConfig memory) {
        return ConfigLoader.loadDeploymentConfig(string.concat(vm.projectRoot(), "/", CONFIG));
    }

    /// @dev Router proxy: `BOUNDLESS_ROUTER`, else the section's `boundless-router`.
    function _router(DeploymentConfig memory deploymentConfig) internal view returns (BoundlessRouter) {
        address routerAddress = vm.envOr("BOUNDLESS_ROUTER", deploymentConfig.boundlessRouter);
        require(routerAddress != address(0), "set boundless-router in deployment.toml or BOUNDLESS_ROUTER");
        return BoundlessRouter(routerAddress);
    }

    function _broadcast() internal {
        uint256 key = vm.envOr("DEPLOYER_PRIVATE_KEY", uint256(0));
        require(key != 0, "DEPLOYER_PRIVATE_KEY must be set");
        vm.rememberKey(key);
        vm.startBroadcast(key);
    }

    /// @dev When true, admin-gated calls are collected into a Safe batch instead of being
    ///      broadcast. Set from `GNOSIS_EXECUTE` by the script before registration runs.
    bool internal _gnosis;
    /// @dev Calldata / labels for the admin-gated calls queued in gnosis mode.
    bytes[] internal _batchData;
    string[] internal _batchLabels;

    /// @dev Apply one admin-gated `router` call. In gnosis mode it is queued for the Safe
    ///      batch (the deployer never sends it); otherwise it is sent inside the active
    ///      broadcast, bubbling any revert reason.
    function _emit(BoundlessRouter router, bytes memory data, string memory label) internal {
        if (_gnosis) {
            _batchData.push(data);
            _batchLabels.push(label);
            console2.log("Queued for Safe batch:", label);
            return;
        }
        (bool ok, bytes memory ret) = address(router).call(data);
        if (!ok) {
            assembly {
                revert(add(ret, 0x20), mload(ret))
            }
        }
        console2.log("Executed:", label);
    }

    /// @dev Writes the queued admin calls as a Safe Transaction Builder JSON (one atomic
    ///      batch the Safe imports and executes). Reverts if nothing was queued — e.g. the
    ///      router is already fully configured.
    function _writeSafeBatch(BoundlessRouter router, address safe) internal virtual {
        uint256 n = _batchData.length;
        require(n != 0, "nothing to batch: router already configured?");

        string[] memory args = new string[](8 + n * 4);
        args[0] = "python3";
        args[1] = "contracts/scripts/write_safe_batch.py";
        args[2] = "--chain-id";
        args[3] = vm.toString(block.chainid);
        args[4] = "--safe";
        args[5] = vm.toString(safe);
        args[6] = "--to";
        args[7] = vm.toString(address(router));
        for (uint256 i = 0; i < n; i++) {
            uint256 base = 8 + i * 4;
            args[base] = "--data";
            args[base + 1] = vm.toString(_batchData[i]);
            args[base + 2] = "--label";
            args[base + 3] = _batchLabels[i];
        }

        bytes memory out = vm.ffi(args);
        console2.log("Wrote Safe batch (%d calls) to:", n);
        console2.log(string(out));
    }

    /// @dev Adds `metadata` as class `classId` unless the id is already a class (skip) or
    ///      tombstoned (skip with warning — a tombstoned id can never be reused).
    function _ensureClass(BoundlessRouter router, bytes4 classId, BoundlessRouter.ClassMetadata memory metadata)
        internal
    {
        (bytes4 existingTag,,,,,,,) = router.classes(classId);
        if (existingTag != bytes4(0)) {
            console2.log("Class already registered, skipping:", metadata.label);
            return;
        }
        if (router.tombstoned(classId)) {
            console2.log("WARNING: class id is tombstoned and cannot be reused:", metadata.label);
            return;
        }
        _emit(
            router,
            abi.encodeCall(BoundlessRouter.addClass, (classId, metadata)),
            string.concat("addClass ", metadata.label)
        );
        console2.logBytes4(classId);
    }

    /// @dev True when `selector` can be registered: not already an entry (skip) and not
    ///      tombstoned (skip with warning).
    function _entryFree(BoundlessRouter router, bytes4 selector, string memory label) internal view returns (bool) {
        (address impl,,) = router.entries(selector);
        if (impl != address(0)) {
            console2.log("Entry already registered, skipping:", label);
            return false;
        }
        if (router.tombstoned(selector)) {
            console2.log("WARNING: selector is tombstoned and cannot be reused:", label);
            return false;
        }
        return true;
    }
}

/// @notice Configure a freshly deployed `BoundlessRouter` in one run: all classes (one per
///         proof type, `R0SetInclusion` as the chain default) and every entry the chain can
///         serve. Idempotent — everything already registered is skipped, so a partial
///         failure is fixed by re-running and a configured router is a no-op.
/// @dev    Entries are registered from two selector kinds: the canonical groth16 /
///         blake3-groth16 selectors (release-coupled constants in `RouterConfig`, skipped
///         when the upstream router does not serve them — localnet dev verifiers use
///         dynamic selectors), and the set verifier's own `SELECTOR()` (guest-version
///         derived, read on-chain).
///
///         Inputs come from the `CHAIN_KEY` section of `deployment.toml`, each
///         overridable by env var:
///           BOUNDLESS_ROUTER    <- boundless-router      router proxy address
///           R0_ROUTER           <- application-verifier  upstream verifier router serving
///                                  the canonical groth16 / blake3-groth16 selectors
///           SET_VERIFIER        <- set-verifier          set-inclusion entry + underlying
///                                  verifier of the R0 assessor adapter
///           ASSESSOR_IMAGE_ID   <- assessor-image-id     bound by the R0 assessor entry
///         DEPLOYER_PRIVATE_KEY — broadcaster; must hold ADMIN_ROLE in the default
///         (EOA-admin) flow, but must NOT in the Safe flow below.
///
///         GNOSIS_EXECUTE=true switches to the Safe flow used when the router was deployed
///         with the Safe as admin (`ROUTER_ADMIN=$SAFE`), so the deployer EOA never holds
///         ADMIN_ROLE. The deployer still broadcasts the (admin-free) adapter deployments —
///         run with `--broadcast` so they land at the addresses the batch references — but
///         the admin-gated `addClass` / `instantiate` calls are written to a Safe
///         Transaction Builder JSON (`contracts/safe-batch/router-bootstrap-<CHAIN_KEY>.json`)
///         for the multisig to import and execute as one atomic batch, instead of being sent.
///         The Safe address resolves from ROUTER_ADMIN / SAFE / the section's `admin`.
contract BootstrapRouter is RouterManageBase {
    function run() external {
        DeploymentConfig memory deploymentConfig = _config();
        BoundlessRouter router = _router(deploymentConfig);
        RiscZeroVerifierRouter r0Router =
            RiscZeroVerifierRouter(vm.envOr("R0_ROUTER", deploymentConfig.applicationVerifier));
        address setVerifier = vm.envOr("SET_VERIFIER", deploymentConfig.setVerifier);
        bytes32 assessorImageId = vm.envOr("ASSESSOR_IMAGE_ID", deploymentConfig.assessorImageId);

        require(address(r0Router) != address(0), "set application-verifier in deployment.toml or R0_ROUTER");
        require(setVerifier != address(0), "set set-verifier in deployment.toml or SET_VERIFIER");
        require(assessorImageId != bytes32(0), "set assessor-image-id in deployment.toml or ASSESSOR_IMAGE_ID");

        _gnosis = vm.envOr("GNOSIS_EXECUTE", false);
        address safe;
        if (_gnosis) {
            // The router must already be admin'd by the Safe (deployed with ROUTER_ADMIN=$SAFE),
            // so the batch we emit is actually executable and the deployer never held the role.
            safe = vm.envOr("ROUTER_ADMIN", vm.envOr("SAFE", deploymentConfig.admin));
            require(safe != address(0), "set ROUTER_ADMIN / SAFE / admin for GNOSIS_EXECUTE");
            require(
                router.hasRole(router.ADMIN_ROLE(), safe),
                "router admin is not the Safe; deploy the router with ROUTER_ADMIN=$SAFE"
            );
            console2.log("GNOSIS_EXECUTE=true: emitting addClass/instantiate as a Safe batch for", safe);
        }

        // Deploys the adapters under the deployer key; admin-gated calls are routed through
        // `_emit` (queued for the Safe batch in gnosis mode, sent here otherwise).
        _broadcast();

        // The assessor class first: verifier classes reference it via
        // `requiredAssessorClass`, which `addClass` validates against existing classes.
        _ensureClass(router, RouterConfig.R0_ASSESSOR_CLASS_ID, RouterConfig.assessorClass());
        _ensureClass(router, RouterConfig.R0_SET_INCLUSION_CLASS_ID, RouterConfig.setInclusionClass());
        _ensureClass(router, RouterConfig.R0_GROTH16_CLASS_ID, RouterConfig.groth16Class());
        _ensureClass(router, RouterConfig.R0_GROTH16_BLAKE3_CLASS_ID, RouterConfig.groth16Blake3Class());

        // Set-inclusion entry at the set verifier's own (set-builder-version-derived) selector.
        // Verify through whatever the upstream router registers for that selector (an
        // emergency-stop wrapper on staging/prod), matching the legacy market; fall back to the
        // bare set verifier on chains where the router does not front it. The selector itself is
        // read from the bare set verifier, since the emergency-stop wrapper has no SELECTOR().
        bytes4 setSelector = IRiscZeroSelectable(setVerifier).SELECTOR();
        IRiscZeroVerifier setInclusionVerifier = _resolveUpstream(r0Router, setSelector, setVerifier);
        if (_entryFree(router, setSelector, "set-inclusion verifier")) {
            R0BoundlessVerifierAdapter adapter = new R0BoundlessVerifierAdapter(setInclusionVerifier);
            _emit(
                router,
                abi.encodeCall(
                    BoundlessRouter.instantiate,
                    (setSelector, address(adapter), RouterConfig.R0_SET_INCLUSION_CLASS_ID, 0)
                ),
                "instantiate set-inclusion verifier"
            );
            console2.log("Set-inclusion verifier adapter at", address(adapter));
            console2.logBytes4(setSelector);
        }

        // Canonical root-proof entries, each in its own proof-type class.
        _ensureUpstreamVerifier(
            router, r0Router, RouterConfig.GROTH16_SELECTOR, RouterConfig.R0_GROTH16_CLASS_ID, "groth16 verifier"
        );
        _ensureUpstreamVerifier(
            router,
            r0Router,
            RouterConfig.GROTH16_BLAKE3_SELECTOR,
            RouterConfig.R0_GROTH16_BLAKE3_CLASS_ID,
            "blake3-groth16 verifier"
        );

        // Both assessor entries under the shared assessor class.
        if (_entryFree(router, RouterConfig.R0_ASSESSOR_SELECTOR, "R0 STARK assessor")) {
            R0BoundlessAssessorAdapter assessorAdapter =
                new R0BoundlessAssessorAdapter(setInclusionVerifier, assessorImageId);
            _emit(
                router,
                abi.encodeCall(
                    BoundlessRouter.instantiate,
                    (RouterConfig.R0_ASSESSOR_SELECTOR, address(assessorAdapter), RouterConfig.R0_ASSESSOR_CLASS_ID, 0)
                ),
                "instantiate R0 STARK assessor"
            );
            console2.log("R0 STARK assessor adapter at", address(assessorAdapter));
        }
        // SKIP_ONCHAIN_ASSESSOR=true defers the on-chain assessor so the R0 guest path can
        // be exercised first (brokers prefer the on-chain assessor whenever its class
        // registers one); re-running the bootstrap without the flag fills in just this entry.
        if (
            !vm.envOr("SKIP_ONCHAIN_ASSESSOR", false)
                && _entryFree(router, RouterConfig.ONCHAIN_ASSESSOR_SELECTOR, "on-chain assessor")
        ) {
            OnChainAssessor onchainAssessor = new OnChainAssessor();
            _emit(
                router,
                abi.encodeCall(
                    BoundlessRouter.instantiate,
                    (
                        RouterConfig.ONCHAIN_ASSESSOR_SELECTOR,
                        address(onchainAssessor),
                        RouterConfig.R0_ASSESSOR_CLASS_ID,
                        0
                    )
                ),
                "instantiate on-chain assessor"
            );
            console2.log("On-chain assessor at", address(onchainAssessor));
        }

        vm.stopBroadcast();

        if (_gnosis) {
            _writeSafeBatch(router, safe);
            console2.log("Import the JSON into the Safe Transaction Builder and execute it as the admin.");
        } else {
            console2.log("Bootstrap complete. Default class:");
            console2.logBytes4(router.defaultClassId());
        }
    }

    /// @dev Registers `selector` under `classId` when the upstream R0 router serves it;
    ///      skips with a log otherwise (e.g. localnet dev verifiers at dynamic selectors).
    function _ensureUpstreamVerifier(
        BoundlessRouter router,
        RiscZeroVerifierRouter r0Router,
        bytes4 selector,
        bytes4 classId,
        string memory label
    ) internal {
        try r0Router.getVerifier(selector) returns (IRiscZeroVerifier underlying) {
            if (_entryFree(router, selector, label)) {
                R0BoundlessVerifierAdapter adapter = new R0BoundlessVerifierAdapter(underlying);
                _emit(
                    router,
                    abi.encodeCall(BoundlessRouter.instantiate, (selector, address(adapter), classId, 0)),
                    string.concat("instantiate ", label)
                );
                console2.log("Verifier adapter at", address(adapter));
                console2.logBytes4(selector);
            }
        } catch {
            console2.log("Upstream router does not serve selector, skipping:", label);
            console2.logBytes4(selector);
        }
    }

    /// @dev The verifier the legacy market uses for `selector`: whatever the upstream R0 router
    ///      registers (an emergency-stop wrapper on staging/prod). Falls back to `fallbackVerifier`
    ///      on chains where the router does not front the selector (e.g. localnet / fresh deploys).
    function _resolveUpstream(RiscZeroVerifierRouter r0Router, bytes4 selector, address fallbackVerifier)
        internal
        view
        returns (IRiscZeroVerifier)
    {
        try r0Router.getVerifier(selector) returns (IRiscZeroVerifier underlying) {
            return underlying;
        } catch {
            return IRiscZeroVerifier(fallbackVerifier);
        }
    }
}

/// @notice Tombstone an entry in the router. Once removed, the bytes4 cannot
///         be reused for any class or impl. Use after a broker rollover when
///         a deprecated assessor or verifier is no longer reachable.
/// @dev    Router from the `CHAIN_KEY` section's `boundless-router` (env-overridable).
///         Required env:
///           DEPLOYER_PRIVATE_KEY   — broadcaster (must hold ADMIN_ROLE)
///           ENTRY_SELECTOR         — bytes4 to tombstone
contract RemoveEntry is RouterManageBase {
    function run() external {
        BoundlessRouter router = _router(_config());
        bytes4 selector = bytes4(vm.envBytes32("ENTRY_SELECTOR"));
        require(selector != bytes4(0), "ENTRY_SELECTOR must be non-zero");

        _broadcast();
        router.removeEntry(selector);
        vm.stopBroadcast();

        console2.log("Tombstoned entry at selector:");
        console2.logBytes4(selector);
    }
}

/// @notice Hand router governance to a Safe / timelock after bring-up: grants
///         ADMIN_ROLE to the new admin and renounces the deployer's role, so the
///         bootstrap can run as a plain EOA and governance receives a configured
///         router. Future class / entry mutations are then admin transactions.
/// @dev    Router from the `CHAIN_KEY` section's `boundless-router` (env-overridable).
///         Required env:
///           DEPLOYER_PRIVATE_KEY   — current admin (the bring-up EOA)
///           NEW_ADMIN              — address receiving ADMIN_ROLE
contract TransferRouterAdmin is RouterManageBase {
    function run() external {
        BoundlessRouter router = _router(_config());
        address newAdmin = vm.envAddress("NEW_ADMIN");
        require(newAdmin != address(0), "NEW_ADMIN must be set");

        uint256 key = vm.envOr("DEPLOYER_PRIVATE_KEY", uint256(0));
        require(key != 0, "DEPLOYER_PRIVATE_KEY must be set");
        address deployer = vm.addr(key);
        require(newAdmin != deployer, "NEW_ADMIN equals the deployer");

        _broadcast();
        router.grantRole(router.ADMIN_ROLE(), newAdmin);
        router.renounceRole(router.ADMIN_ROLE(), deployer);
        vm.stopBroadcast();

        require(router.hasRole(router.ADMIN_ROLE(), newAdmin), "new admin did not receive ADMIN_ROLE");
        require(!router.hasRole(router.ADMIN_ROLE(), deployer), "deployer still holds ADMIN_ROLE");
        console2.log("Router ADMIN_ROLE transferred to", newAdmin);
    }
}
