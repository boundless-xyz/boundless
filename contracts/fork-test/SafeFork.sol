// Copyright 2026 Boundless Foundation, Inc.
//
// Use of this source code is governed by the Business Source License
// as found in the LICENSE-BSL file.

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

    struct Reference {
        uint256 length;
        uint256 start;
    }
    error RuntimeMismatch(address candidate);

    function requireExisting(address candidate) internal view {
        if (candidate.code.length == 0) revert RuntimeMismatch(candidate);
    }

    /// Compare every emitted runtime byte and all constructor dependencies.
    /// Only the compiler-listed UUPS __self references may differ by deployment address.
    function checkRuntime(address local, address candidate, string memory artifact) internal view {
        requireExisting(candidate);
        bytes memory expected = local.code;
        bytes memory actual = candidate.code;
        if (candidate == local) return;
        if (expected.length != actual.length) revert RuntimeMismatch(candidate);
        string memory json = vm.readFile(artifact);
        string memory key = ".deployedBytecode.immutableReferences";
        uint256 count = vm.parseJsonKeys(json, key).length;
        bytes memory object = vm.parseJson(json, key);
        // Foundry encodes the object as a tuple of arrays. Add the outer array length
        // so every immutable group can be decoded after parsing the large artifact once.
        bytes memory encoded = new bytes(object.length + 32);
        assembly {
            mstore(add(encoded, 32), 32)
            mstore(add(encoded, 64), count)
            for { let offset := 32 } lt(offset, mload(object)) { offset := add(offset, 32) } {
                mstore(add(add(encoded, 64), offset), mload(add(add(object, 32), offset)))
            }
        }
        Reference[][] memory groups = abi.decode(encoded, (Reference[][]));
        bool foundSelf;
        for (uint256 i; i < groups.length; ++i) {
            Reference[] memory refs = groups[i];
            for (uint256 j; j < refs.length; ++j) {
                uint256 offset = refs[j].start;
                require(refs[j].length == 32 && offset + 32 <= expected.length, "invalid immutable reference");
                bytes32 value;
                assembly { value := mload(add(add(expected, 32), offset)) }
                if (value == bytes32(uint256(uint160(local)))) {
                    foundSelf = true;
                    bytes32 replacement = bytes32(uint256(uint160(candidate)));
                    assembly { mstore(add(add(expected, 32), offset), replacement) }
                }
            }
        }
        require(foundSelf, "missing UUPS self reference");
        if (keccak256(expected) != keccak256(actual)) revert RuntimeMismatch(candidate);
    }

    function upgradeCall(address candidate) internal pure returns (bytes memory) {
        return abi.encodeWithSignature("upgradeToAndCall(address,bytes)", candidate, bytes(""));
    }

    function packedCall(address target, bytes memory data) internal pure returns (bytes memory) {
        return abi.encodePacked(uint8(0), target, uint256(0), uint256(data.length), data);
    }

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
