// Copyright 2026 Boundless Foundation, Inc.
//
// Use of this source code is governed by the Business Source License
// as found in the LICENSE-BSL file.

pragma solidity ^0.8.26;

import {Script, console2} from "forge-std/Script.sol";
import {ISunsetSafe, SunsetTransaction} from "./SunsetTransaction.sol";

/// Runs locally against RPC state. No broadcast or signing-key access.
abstract contract SunsetPrinter is Script {
    function printTransaction(SunsetTransaction.Transaction memory transaction) internal {
        ISunsetSafe safe = ISunsetSafe(transaction.safe);
        address[] memory owners = safe.getOwners();
        uint256 threshold = safe.getThreshold();
        require(threshold == 3 && owners.length == 6, "unexpected Safe configuration");
        uint256 nonce = safe.nonce();
        bytes32 hash = safe.getTransactionHash(
            transaction.to, 0, transaction.data, transaction.operation, 0, 0, 0, address(0), address(0), nonce
        );
        string memory object = "sunset";
        vm.serializeUint(object, "chainId", block.chainid);
        vm.serializeUint(object, "blockNumber", block.number);
        vm.serializeAddress(object, "safe", transaction.safe);
        vm.serializeAddress(object, "to", transaction.to);
        vm.serializeUint(object, "value", 0);
        vm.serializeUint(object, "operation", transaction.operation);
        vm.serializeUint(object, "safeTxGas", 0);
        vm.serializeUint(object, "baseGas", 0);
        vm.serializeUint(object, "gasPrice", 0);
        vm.serializeAddress(object, "gasToken", address(0));
        vm.serializeAddress(object, "refundReceiver", address(0));
        vm.serializeUint(object, "nonce", nonce);
        vm.serializeAddress(object, "owners", owners);
        vm.serializeUint(object, "threshold", threshold);
        vm.serializeBytes32(object, "safeTxHash", hash);
        console2.log(string.concat("SUNSET_TX_JSON=", vm.serializeBytes(object, "data", transaction.data)));
    }
}
