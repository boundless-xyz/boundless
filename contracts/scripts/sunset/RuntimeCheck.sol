// Copyright 2026 Boundless Foundation, Inc.
//
// Use of this source code is governed by the Business Source License
// as found in the LICENSE-BSL file.

pragma solidity ^0.8.26;

import {Vm} from "forge-std/Vm.sol";

library RuntimeCheck {
    Vm private constant VM = Vm(address(uint160(uint256(keccak256("hevm cheat code")))));

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
    function check(address local, address candidate, string memory artifact) internal view {
        requireExisting(candidate);
        bytes memory expected = local.code;
        bytes memory actual = candidate.code;
        if (candidate == local) return;
        string memory json = VM.readFile(artifact);
        string memory key = ".deployedBytecode.immutableReferences";
        string[] memory ids = VM.parseJsonKeys(json, key);
        bool foundSelf;
        for (uint256 i; i < ids.length; ++i) {
            Reference[] memory refs = abi.decode(VM.parseJson(json, string.concat(key, ".", ids[i])), (Reference[]));
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
}
