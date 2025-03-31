// Copyright (c) 2025 RISC Zero, Inc.
//
// All rights reserved.

pragma solidity ^0.8.13;

import {ICounter} from "./ICounter.sol";
import {IRiscZeroVerifier} from "risc0/IRiscZeroVerifier.sol";
import {BoundlessMarketCallback} from "boundless-market/BoundlessMarketCallback.sol";

error AlreadyVerified(bytes32 received);

// @notice Counter is a simple contract that increments a counter for each verified callback.
// @dev It inherits from BoundlessMarketCallback to handle proofs delivered by the Boundless Market.
contract Counter is ICounter, BoundlessMarketCallback {
    uint256 public count;

    constructor(IRiscZeroVerifier verifier, address boundlessMarket, bytes32 imageId)
        BoundlessMarketCallback(verifier, boundlessMarket, imageId)
    {
        count = 0;
    }

    // @notice Increments the counter and emits an event when a proof is verified.
    // @dev This function is called by the Boundless Market when a proof is verified.
    // @param imageId The ID of the image that was verified.
    // @param journal The journal from the zkVM program.
    // @param seal The seal from the zkVM program.
    function _handleProof(bytes32 imageId, bytes calldata journal, bytes calldata seal) internal override {
        count += 1;

        emit CounterCallbackCalled(imageId, journal, seal);
    }
}
