// Copyright 2025 RISC Zero, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

pragma solidity ^0.8.13;

import {IRiscZeroVerifier} from "risc0/IRiscZeroVerifier.sol";
import {BoundlessMarketCallback} from "boundless-market/BoundlessMarketCallback.sol";
import {ICounter} from "./ICounter.sol";

error AlreadyVerified(bytes32 received);

/// @notice Counter is a simple contract that increments a counter for each verified callback.
/// @dev It inherits from BoundlessMarketCallback to handle proofs delivered by the Boundless Market.
contract Counter is ICounter, BoundlessMarketCallback {
    uint256 public count;
    /// @notice Mapping to track verified proofs.
    /// @dev This is used to prevent a callback is called more than once with the same proof.
    mapping(bytes32 => bool) public verified;

    constructor(IRiscZeroVerifier verifier, address boundlessMarket, bytes32 imageId)
        BoundlessMarketCallback(verifier, boundlessMarket, imageId)
    {
        count = 0;
    }

    /// @notice Increments the counter and emits an event when a new proof is verified.
    /// @dev This function is called by the Boundless Market when a proof is verified.
    /// @param imageId The ID of the RISC Zero guest image that produced the proof
    /// @param journal The output journal from the RISC Zero guest execution
    /// @param seal The cryptographic seal proving correct execution
    function _handleProof(bytes32 imageId, bytes calldata journal, bytes calldata seal) internal override {
        // Since a callback can be triggered by any requestor sending a valid request to the Boundless Market,
        // we need to perform some checks on the proof before proceeding.
        // First, the validation of the proof (e.g., seal is valid, the caller of the callback is the BoundlessMarket)
        // is done in the parent contract, the `BoundlessMarketCallback`.
        // Here we can add additional checks if needed.
        // For example, we can check if the proof has already been verified,
        // so that the same proof cannot be used more than once to run the callback logic.
        bytes32 journalAndSeal = keccak256(abi.encode(journal, seal));
        if (verified[journalAndSeal]) {
            revert AlreadyVerified();
        }
        // Mark the proof as verified.
        verified[journalAndSeal] = true;

        // run the callback logic
        count += 1;
        emit CounterCallbackCalled(imageId, journal, seal);
    }
}
