// Copyright (c) 2025 RISC Zero, Inc.
//
// All rights reserved.

pragma solidity ^0.8.24;

import {IERC20} from "openzeppelin/contracts/token/ERC20/ERC20.sol";

interface IERC20Mint is IERC20 {
    /// A sender-authorized mint function as a placeholder for a real minting mechanism.
    function mint(address to, uint256 value) external;
}
