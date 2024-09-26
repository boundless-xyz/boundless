// Copyright (c) 2024 RISC Zero, Inc.
//
// All rights reserved.

pragma solidity ^0.8.13;

// Counter is a simple contract that increments a counter for each address that calls increment.
// The increment function takes a seal and a journal digest, where the seal contains the proof of inclusion
// (empty in case of singleton proofs) and verifies it using the CrossDomainSetOfTruth deployed on L1.
// If the verification is successful, the journal digest is marked as verified,
// so that can't be reused, and the counter is incremented.
// It could be used to play a sort of game where each player has to submit as many unique proofs
// coming from the market to increment their counter.
interface ICounter {
    event Increment(address indexed who, uint256 count);

    function increment(bytes calldata seal, bytes32 imageId, bytes32 journalDigest) external;
    function getCount(address who) external view returns (uint256);
}
