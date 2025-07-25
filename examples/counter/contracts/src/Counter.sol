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

import {ICounter} from "./ICounter.sol";
import {IRiscZeroVerifier} from "risc0/IRiscZeroVerifier.sol";

error AlreadyVerified(bytes32 received);

// Counter is a simple contract that increments a counter for each address that calls increment.
// The increment function takes a seal and a journal digest, where the seal contains the proof of inclusion
// (empty in case of singleton proofs) and verifies it using the SetVerifier contract.
// If the verification is successful, the journal digest is marked as verified,
// so that can't be reused, and the counter is incremented.
// It could be used to play a sort of game where each player has to submit as many unique proofs
// coming from the market to increment their counter.
contract Counter is ICounter {
    mapping(address => uint256) public count;
    mapping(bytes32 => bool) public verified;
    IRiscZeroVerifier public immutable VERIFIER;

    constructor(IRiscZeroVerifier verifier) {
        VERIFIER = verifier;
    }

    function increment(bytes calldata seal, bytes32 imageId, bytes32 journalDigest) public {
        if (verified[journalDigest]) {
            revert AlreadyVerified({received: journalDigest});
        }

        VERIFIER.verify(seal, imageId, journalDigest);
        verified[journalDigest] = true;
        count[msg.sender] += 1;
        emit Increment(msg.sender, count[msg.sender]);
    }

    function getCount(address who) public view returns (uint256) {
        return count[who];
    }
}
