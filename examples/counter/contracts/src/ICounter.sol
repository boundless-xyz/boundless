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

// Counter is a simple contract that increments a counter for each address that calls increment.
// The increment function takes a seal and a journal digest, where the seal contains the proof of inclusion
// (empty in case of singleton proofs) and verifies it using the SetVerifier contract.
// If the verification is successful, the journal digest is marked as verified,
// so that can't be reused, and the counter is incremented.
// It could be used to play a sort of game where each player has to submit as many unique proofs
// coming from the market to increment their counter.
interface ICounter {
    event Increment(address indexed who, uint256 count);

    function increment(bytes calldata seal, bytes32 imageId, bytes32 journalDigest) external;
    function getCount(address who) external view returns (uint256);
}
