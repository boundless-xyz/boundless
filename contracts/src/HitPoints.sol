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

pragma solidity ^0.8.20;

import "@openzeppelin/contracts/token/ERC20/ERC20.sol";
import "@openzeppelin/contracts/token/ERC20/extensions/ERC20Burnable.sol";
import "@openzeppelin/contracts/token/ERC20/extensions/ERC20Permit.sol";
import "@openzeppelin/contracts/access/Ownable.sol";

import "./IHitPoints.sol";

/// @title HitPoints ERC20
/// @notice Implementation of a restricted transfer token using ERC20
contract HitPoints is ERC20, ERC20Burnable, ERC20Permit, IHitPoints, Ownable {
    // Maximum allowed balance (uint96 max value)
    uint256 private constant MAX_BALANCE = type(uint96).max;
    // Mapping for accounts that are authorized as either source or destination of a transfer.
    mapping(address => bool) public isAuthorized;

    constructor(address initialOwner) ERC20("HitPoints", "HP") ERC20Permit("HitPoints") Ownable(initialOwner) {
        // Authorize address(0) as a sender and receiver to simplify mints and burns.
        isAuthorized[address(0)] = true;
    }

    modifier onlyAuthorized() {
        if (!isAuthorized[msg.sender] && msg.sender != owner()) revert Unauthorized();
        _;
    }

    /// @inheritdoc IHitPoints
    function authorize(address operator) external onlyOwner {
        isAuthorized[operator] = true;
    }

    /// @inheritdoc IHitPoints
    function deauthorize(address operator) external onlyOwner {
        isAuthorized[operator] = false;
    }

    /// @inheritdoc IHitPoints
    function mint(address account, uint256 value) external onlyOwner {
        // Ensure the new balance won't exceed uint96 max
        uint256 newBalance = balanceOf(account) + value;
        if (newBalance > MAX_BALANCE) {
            revert BalanceExceedsLimit(account, balanceOf(account), value);
        }
        _mint(account, value);
    }

    function _update(address from, address to, uint256 value) internal virtual override {
        // Either the sender or the receiver must be authorized.
        if (!isAuthorized[from] && !isAuthorized[to]) {
            revert UnauthorizedTransfer();
        }

        // Prevent direct transfers to authorized addresses
        // Allow transfers only through allowance or permit
        if (isAuthorized[to] && from != address(0)) {
            if (allowance(from, msg.sender) < value && msg.sender != to && !isAuthorized[from]) {
                revert UnauthorizedTransfer();
            }
        }

        super._update(from, to, value);

        // Ensure the recipient's balance didn't exceed MAX_BALANCE.
        if (to != address(0) && balanceOf(to) > MAX_BALANCE) {
            revert BalanceExceedsLimit(to, balanceOf(to) - value, value);
        }
    }
}
