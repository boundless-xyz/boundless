// Copyright (c) 2025 RISC Zero, Inc.
//
// All rights reserved.

pragma solidity ^0.8.24;

import {ERC20} from "@openzeppelin/contracts/token/ERC20/ERC20.sol";
import {ERC20Permit} from "@openzeppelin/contracts/token/ERC20/extensions/ERC20Permit.sol";
import {IERC20Mint} from "../src/povw/IERC20Mint.sol";

contract MockERC20Mint is IERC20Mint, ERC20, ERC20Permit {
    constructor() ERC20("MockERC20Mint", "MOCK") ERC20Permit("MockERC20Mint") {}

    function mint(address to, uint256 value) public virtual {
        _mint(to, value);
    }
}
