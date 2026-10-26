// Copyright 2026 Boundless Foundation, Inc.
//
// Use of this source code is governed by the Business Source License
// as found in the LICENSE-BSL file.

pragma solidity ^0.8.26;

interface ISunsetSafe {
    function getOwners() external view returns (address[] memory);
    function getThreshold() external view returns (uint256);
    function nonce() external view returns (uint256);
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
}

/// The printer and fork tests use these exact transaction builders.
library SunsetTransaction {
    struct Transaction {
        address safe;
        address to;
        uint8 operation;
        bytes data;
    }

    function upgrade(address implementation) internal pure returns (bytes memory) {
        return abi.encodeWithSignature("upgradeToAndCall(address,bytes)", implementation, bytes(""));
    }

    function single(address safe, address proxy, address implementation) internal pure returns (Transaction memory) {
        return Transaction(safe, proxy, 0, upgrade(implementation));
    }

    function pair(address safe, address multiSend, address proxy1, address impl1, address proxy2, address impl2)
        internal
        pure
        returns (Transaction memory)
    {
        bytes memory first = upgrade(impl1);
        bytes memory second = upgrade(impl2);
        bytes memory packed = abi.encodePacked(
            uint8(0),
            proxy1,
            uint256(0),
            uint256(first.length),
            first,
            uint8(0),
            proxy2,
            uint256(0),
            uint256(second.length),
            second
        );
        return Transaction(safe, multiSend, 1, abi.encodeWithSignature("multiSend(bytes)", packed));
    }
}
