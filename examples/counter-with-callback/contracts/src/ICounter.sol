// Copyright (c) 2024 RISC Zero, Inc.
//
// All rights reserved.

pragma solidity ^0.8.13;

import {IBoundlessMarketCallback} from "../../../../contracts/src/IBoundlessMarketCallback.sol";

// @notice ICounter is a simple interface that inherits from IBoundlessMarketCallback 
// to handle proofs delivered by the Boundless Market.
interface ICounter is IBoundlessMarketCallback {
    // @notice Emitted when the counter is incremented.
    event CounterCallbackCalled(bytes32 imageId, bytes journal, bytes seal);
    // @notice Retrieves the current count.
    // @return The current count.
    function getCount() external view returns (uint256);
}
