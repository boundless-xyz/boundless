// Copyright (c) 2025 RISC Zero, Inc.
//
// All rights reserved.
pragma solidity ^0.8.24;

import {IRiscZeroVerifier} from "risc0/IRiscZeroVerifier.sol";
import {BoundlessMarketCallback} from "../src/BoundlessMarketCallback.sol";

/// @notice Mock callback contract for testing BoundlessMarket callbacks
/// @dev This contract allows configuring how much gas the callback should consume
contract MockCallback is BoundlessMarketCallback {
    uint256 public callCount;
    uint256 public targetGas;

    event MockCallbackCalled(bytes32 imageId, bytes journal, bytes seal);

    // Store info about each call
    struct CallInfo {
        bytes32 imageId;
        bytes journal;
        bytes seal;
    }

    // Mapping used for mocking gas consumption
    mapping(bytes32 => uint256) private gasConsumptionSlots;

    constructor(IRiscZeroVerifier verifier, address boundlessMarket, uint256 _targetGas)
        BoundlessMarketCallback(verifier, boundlessMarket)
    {
        targetGas = _targetGas;
    }

    function _handleProof(bytes32 imageId, bytes calldata journal, bytes calldata seal) internal override {
        uint256 startGas = gasleft();

        emit MockCallbackCalled(imageId, journal, seal);
        callCount++;

        // Consume gas by doing SSTORE operations to random slots
        uint256 i = 0;
        while (startGas - gasleft() < targetGas) {
            bytes32 slot = keccak256(abi.encode(i));
            gasConsumptionSlots[slot] = i;
            i++;
        }
    }

    function getCallCount() external view returns (uint256) {
        return callCount;
    }
}
