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
import "@openzeppelin/contracts/token/ERC20/extensions/ERC20Permit.sol";
import "@openzeppelin/contracts/access/Ownable.sol";

import "./IHitPoints.sol";

/// @title HitPoints ERC20
/// @notice Implementation of a restricted transfer token using ERC20
contract HitPoints is ERC20, ERC20Permit, IHitPoints, Ownable {
    // Maximum allowed balance (uint96 max value)
    uint256 private constant MAX_BALANCE = type(uint96).max;
    // Mapping for operators who can mint and can receive/send tokens from/to anyone
    mapping(address => bool) public isAuthorized;

    constructor(address initialOwner) ERC20("HitPoints", "HP") ERC20Permit("HitPoints") Ownable(initialOwner) {
        isAuthorized[initialOwner] = true;
    }

    modifier onlyAuthorized() {
        if (!isAuthorized[msg.sender]) revert Unauthorized();
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
    function mint(address account, uint256 value) external onlyAuthorized {
        // Ensure the new balance won't exceed uint96 max
        uint256 newBalance = balanceOf(account) + value;
        if (newBalance > MAX_BALANCE) {
            revert BalanceExceedsLimit(account, balanceOf(account), value);
        }
        _mint(account, value);
    }

    /// @inheritdoc IHitPoints
    function burn(uint256 value) external {
        _burn(msg.sender, value);
    }

    function _update(address from, address to, uint256 value) internal virtual override {
        // Allow mint operations
        if (from == address(0)) {
            super._update(from, to, value);
            return;
        }

        // Ensure the recipient's balance won't exceed uint96 max for non-zero transfers
        if (to != address(0)) {
            uint256 newBalance = balanceOf(to) + value;
            if (newBalance > MAX_BALANCE) {
                revert BalanceExceedsLimit(to, balanceOf(to), value);
            }
        }

        // Authorized accounts can transfer anywhere
        if (isAuthorized[from]) {
            super._update(from, to, value);
            return;
        }

        // Others can only transfer to authorized accounts
        if (!isAuthorized[to]) revert UnauthorizedTransfer();

        super._update(from, to, value);
    }
}
