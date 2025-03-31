// Copyright (c) 2024 RISC Zero, Inc.
//
// All rights reserved.

pragma solidity ^0.8.13;

import {ICounter} from "./ICounter.sol";
import {IRiscZeroVerifier} from "risc0/IRiscZeroVerifier.sol";
import {BoundlessMarketCallback} from "../../../../contracts/src/BoundlessMarketCallback.sol";

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

    function _handleProof(bytes32 imageId, bytes calldata journal, bytes calldata seal) internal override {
        count += 1;

        emit CounterCallbackCalled(imageId, journal, seal);
    }

    function getCount() public view returns (uint256) {
        return count;
    }
}
