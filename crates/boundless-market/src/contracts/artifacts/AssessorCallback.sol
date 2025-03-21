// Copyright (c) 2025 RISC Zero Inc,
//
// Use of this source code is governed by the Business Source License
// as found in the LICENSE-BSL file.
pragma solidity ^0.8.20;

struct AssessorCallback {
    /// @notice The index of the fill in the request
    uint16 index;
    /// @notice The address of the contract to call back
    address addr;
    /// @notice Maximum gas to use for the callback
    uint96 gasLimit;
}
