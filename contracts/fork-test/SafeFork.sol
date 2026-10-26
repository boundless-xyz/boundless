// SPDX-License-Identifier: MIT
pragma solidity ^0.8.26;

import {Test} from "forge-std/Test.sol";

interface IForkSafe {
    function getOwners() external view returns (address[] memory);
    function getThreshold() external view returns (uint256);
    function nonce() external view returns (uint256);
    function approveHash(bytes32) external;
    function getTransactionHash(
        address,
        uint256,
        bytes calldata,
        uint8,
        uint256,
        uint256,
        uint256,
        address,
        address,
        uint256
    ) external view returns (bytes32);
    function execTransaction(
        address,
        uint256,
        bytes calldata,
        uint8,
        uint256,
        uint256,
        uint256,
        address,
        address payable,
        bytes calldata
    ) external payable returns (bool);
}

/// Real Safe threshold and execution path; owners are impersonated only inside the fork.
abstract contract SafeFork is Test {
    bytes32 internal constant IMPLEMENTATION_SLOT = 0x360894a13ba1a3210667c828492db98dca3e2076cc3735a920a3ca505d382bbc;
    uint256 internal constant T = 1793836800;

    function implementation(address proxy) internal view returns (address) {
        return address(uint160(uint256(vm.load(proxy, IMPLEMENTATION_SLOT))));
    }

    function executeSafe(address safeAddress, address target, bytes memory data, uint8 operation, bool expectSuccess)
        internal
    {
        IForkSafe safe = IForkSafe(safeAddress);
        address[] memory owners = safe.getOwners();
        uint256 threshold = safe.getThreshold();
        assertEq(threshold, 3, "unexpected Safe threshold");
        assertEq(owners.length, expectedOwnerCount(), "unexpected Safe owners");
        for (uint256 i; i < owners.length; ++i) {
            for (uint256 j = i + 1; j < owners.length; ++j) {
                if (owners[j] < owners[i]) (owners[i], owners[j]) = (owners[j], owners[i]);
            }
        }
        uint256 nonce = safe.nonce();
        // Nonzero safeTxGas lets the real Safe return false for an intentionally failing batch.
        uint256 safeTxGas = expectSuccess ? 0 : 5_000_000;
        bytes32 hash =
            safe.getTransactionHash(target, 0, data, operation, safeTxGas, 0, 0, address(0), address(0), nonce);
        bytes memory signatures;
        for (uint256 i; i < threshold; ++i) {
            vm.prank(owners[i]);
            safe.approveHash(hash);
            signatures = abi.encodePacked(signatures, bytes32(uint256(uint160(owners[i]))), bytes32(0), uint8(1));
        }
        assertEq(
            safe.execTransaction(
                target, 0, data, operation, safeTxGas, 0, 0, address(0), payable(address(0)), signatures
            ),
            expectSuccess
        );
        assertEq(safe.nonce(), nonce + 1);
    }

    function expectedOwnerCount() internal view virtual returns (uint256) {
        return 6;
    }

    function assertOnlyImplementationWritten(address proxy) internal {
        (, bytes32[] memory written) = vm.accesses(proxy);
        assertGt(written.length, 0, "upgrade did not write proxy");
        for (uint256 i; i < written.length; ++i) {
            assertEq(written[i], IMPLEMENTATION_SLOT, "unexpected proxy storage write");
        }
    }
}
