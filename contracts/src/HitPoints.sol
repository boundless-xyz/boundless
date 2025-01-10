// Copyright (c) 2025 RISC Zero, Inc.
//
// All rights reserved.

pragma solidity ^0.8.24;

import "@openzeppelin/contracts/access/Ownable.sol";
import {IHitPoints} from "./IHitPoints.sol";

/// @title HitPoints
/// @notice Implementation of a non-transferrable token with locking mechanism
contract HitPoints is IHitPoints, Ownable {
    /// @dev Token balances
    mapping(address => uint256) private _balances;

    /// @dev Authorization mapping
    mapping(address => bool) public isAuthorized;

    /// @notice Creates a new HitPoints contract
    /// @param initialOwner The address that will own the contract
    constructor(address initialOwner) Ownable(initialOwner) {
        isAuthorized[initialOwner] = true;
    }

    /// @notice Modifier to restrict function access to authorized addresses
    /// @dev Throws UnauthorizedCaller if caller is not authorized
    modifier onlyAuthorized() {
        if (!isAuthorized[msg.sender]) revert UnauthorizedCaller();
        _;
    }

    /// @notice Adds a new address to the authorized list
    /// @param account The address to authorize
    function authorize(address account) external onlyOwner {
        isAuthorized[account] = true;
    }

    /// @notice Removes an address from the authorized list
    /// @param account The address to remove authorization from
    function deauthorize(address account) external onlyOwner {
        isAuthorized[account] = false;
    }

    /// @notice Creates new tokens and assigns them to an account
    /// @param account The address that will receive the minted tokens
    /// @param amount The amount of tokens to mint
    function mint(address account, uint256 amount) external onlyAuthorized {
        _balances[account] += amount;
    }
    /// @notice Burns tokens from an account
    /// @param account The address to burn tokens from
    /// @param amount The amount of tokens to burn

    function burn(address account, uint256 amount) external onlyAuthorized {
        if (_balances[account] < amount) revert InsufficientBalance(account);

        unchecked {
            _balances[account] -= amount;
        }
    }

    /// @notice Gets the balance of an account
    /// @param account The address to query
    /// @return balance The number of tokens available
    function balanceOf(address account) external view returns (uint256 balance) {
        balance = _balances[account];
    }
}
