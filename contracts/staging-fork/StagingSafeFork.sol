// SPDX-License-Identifier: MIT
pragma solidity ^0.8.26;
import {SafeFork, IForkSafe} from "./SafeFork.sol";

abstract contract StagingSafeFork is SafeFork {
    string internal bundleJson;
    address internal stagingSafe;
    address internal bundleTarget;
    bytes internal bundleData;
    uint8 internal bundleOperation;

    function loadStagingBundle() internal {
        bundleJson = vm.readFile(vm.envString("SUNSET_STAGING_BUNDLE"));
        assertEq(vm.parseJsonUint(bundleJson, ".chainId"), block.chainid);
        assertEq(vm.parseJsonUint(bundleJson, ".blockNumber"), block.number);
        stagingSafe = vm.parseJsonAddress(bundleJson, ".safe");
        address expected = block.chainid == 167000
            ? 0x8E537AAeddbC98a9a095397aDb7c0d738458Cc8b
            : 0xB04D1a222789A76E74168a919b43b20F66e24f0b;
        require(
            block.chainid == 84532 || block.chainid == 167000 || block.chainid == 11155111, "staging chain required"
        );
        assertEq(stagingSafe, expected);
        bundleTarget = vm.parseJsonAddress(bundleJson, ".to");
        bundleData = vm.parseJsonBytes(bundleJson, ".data");
        bundleOperation = uint8(vm.parseJsonUint(bundleJson, ".operation"));
        for (uint256 i; i < 4; ++i) {
            string[4] memory keys = [string(".value"), ".safeTxGas", ".baseGas", ".gasPrice"];
            assertEq(vm.parseJsonUint(bundleJson, keys[i]), 0);
        }
        assertEq(vm.parseJsonAddress(bundleJson, ".gasToken"), address(0));
        assertEq(vm.parseJsonAddress(bundleJson, ".refundReceiver"), address(0));
        IForkSafe safe = IForkSafe(stagingSafe);
        assertEq(safe.getThreshold(), 3);
        assertEq(safe.getOwners().length, block.chainid == 11155111 ? 5 : 6);
        assertEq(vm.parseJsonUint(bundleJson, ".nonce"), safe.nonce());
        assertEq(
            vm.parseJsonBytes32(bundleJson, ".safeTxHash"),
            safe.getTransactionHash(
                bundleTarget, 0, bundleData, bundleOperation, 0, 0, 0, address(0), address(0), safe.nonce()
            )
        );
    }

    function stageAddress(uint256 i, string memory field) internal view returns (address) {
        return vm.parseJsonAddress(bundleJson, string.concat(".steps[", vm.toString(i), "].", field));
    }

    function checkStage(uint256 i, address expectedProxy) internal view returns (address candidate) {
        assertEq(stageAddress(i, "proxy"), expectedProxy);
        address baseline = stageAddress(i, "baseline");
        assertEq(implementation(expectedProxy), baseline);
        assertEq(
            baseline.codehash,
            vm.parseJsonBytes32(bundleJson, string.concat(".steps[", vm.toString(i), "].baselineCodeHash"))
        );
        candidate = stageAddress(i, "implementation");
        assertGt(candidate.code.length, 0);
        assertTrue(candidate != baseline);
    }

    function executeStaging(address target, bytes memory data, uint8 operation, bool success) internal {
        IForkSafe safe = IForkSafe(stagingSafe);
        address[] memory owners = safe.getOwners();
        uint256 threshold = safe.getThreshold();
        assertEq(threshold, 3);
        assertEq(owners.length, block.chainid == 11155111 ? 5 : 6);
        for (uint256 i; i < owners.length; ++i) {
            for (uint256 j = i + 1; j < owners.length; ++j) {
                if (owners[j] < owners[i]) (owners[i], owners[j]) = (owners[j], owners[i]);
            }
        }
        uint256 nonce = safe.nonce();
        uint256 gasLimit = success ? 0 : 5_000_000;
        bytes32 hash =
            safe.getTransactionHash(target, 0, data, operation, gasLimit, 0, 0, address(0), address(0), nonce);
        bytes memory signatures;
        for (uint256 i; i < threshold; ++i) {
            vm.prank(owners[i]);
            safe.approveHash(hash);
            signatures = abi.encodePacked(signatures, bytes32(uint256(uint160(owners[i]))), bytes32(0), uint8(1));
        }
        assertEq(
            safe.execTransaction(
                target, 0, data, operation, gasLimit, 0, 0, address(0), payable(address(0)), signatures
            ),
            success
        );
        assertEq(safe.nonce(), nonce + 1);
    }

    function executeBundle() internal {
        executeStaging(bundleTarget, bundleData, bundleOperation, true);
    }
}
