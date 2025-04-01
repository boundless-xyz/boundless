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

import {AccessControl} from "@openzeppelin/contracts/access/AccessControl.sol";
import "@openzeppelin/contracts/token/ERC20/ERC20.sol";
import "@openzeppelin/contracts/token/ERC20/extensions/ERC20Burnable.sol";
import "@openzeppelin/contracts/token/ERC20/extensions/ERC20Permit.sol";
import "@openzeppelin/contracts/access/Ownable.sol";

import "./IHitPoints.sol";

/// @title HitPoints ERC20
/// @notice Implementation of a restricted transfer token using ERC20
contract HitPoints is ERC20, ERC20Burnable, ERC20Permit, IHitPoints, AccessControl, Ownable {
    // Maximum allowed balance (uint96 max value)
    uint256 constant MAX_BALANCE = type(uint96).max;
    // Role identifier for minting operation
    bytes32 public constant MINTER = keccak256("MINTER");
    // Role identifier for authorized transfer
    bytes32 public constant AUTHORIZED_TRANSFER = keccak256("AUTHORIZED_TRANSFER");

    constructor(address initialOwner) ERC20("HitPoints", "HP") ERC20Permit("HitPoints") Ownable(initialOwner) {
        _grantRole(DEFAULT_ADMIN_ROLE, initialOwner);
        // Authorize address(0) as a sender and receiver to simplify mints and burns.
        _grantRole(AUTHORIZED_TRANSFER, address(0));
    }

    /// @inheritdoc Ownable
    function transferOwnership(address newOwner) public override onlyOwner {
        _revokeRole(DEFAULT_ADMIN_ROLE, owner());
        _grantRole(DEFAULT_ADMIN_ROLE, newOwner);
        super.transferOwnership(newOwner);
    }

    /// @inheritdoc IHitPoints
    function grantMinterRole(address account) external onlyOwner {
        _grantRole(MINTER, account);
    }

    /// @inheritdoc IHitPoints
    function revokeMinterRole(address account) external onlyOwner {
        _revokeRole(MINTER, account);
    }

    /// @inheritdoc IHitPoints
    function grantAuthorizedTransferRole(address account) external onlyOwner {
        _grantRole(AUTHORIZED_TRANSFER, account);
    }

    /// @inheritdoc IHitPoints
    function revokeAuthorizedTransferRole(address account) external onlyOwner {
        _revokeRole(AUTHORIZED_TRANSFER, account);
    }

    /// @inheritdoc IHitPoints
    function mint(address account, uint256 value) external onlyRole(MINTER) {
        _mint(account, value);
    }

    function _update(address from, address to, uint256 value) internal virtual override {
        // Either the sender or the receiver must be authorized.
        if (!hasRole(AUTHORIZED_TRANSFER, from) && !hasRole(AUTHORIZED_TRANSFER, to)) {
            revert UnauthorizedTransfer();
        }

        super._update(from, to, value);

        // Ensure the recipient's balance didn't exceed MAX_BALANCE.
        if (to != address(0) && balanceOf(to) > MAX_BALANCE) {
            revert BalanceExceedsLimit(to, balanceOf(to) - value, value);
        }
    }
}
