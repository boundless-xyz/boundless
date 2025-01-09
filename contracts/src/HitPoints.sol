// Copyright (c) 2025 RISC Zero, Inc.
//
// All rights reserved.

pragma solidity ^0.8.24;

import "@openzeppelin/contracts/access/Ownable.sol";
import {SafeCast} from "@openzeppelin/contracts/utils/math/SafeCast.sol";
import {IHitPoints} from "./IHitPoints.sol";

/// @title HitPoints
/// @notice Implementation of a non-transferrable token with locking mechanism
contract HitPoints is IHitPoints, Ownable {
    using SafeCast for uint256;

    /// @dev Packs available and locked balances into a single storage slot
    struct Balance {
        uint128 available;
        uint128 locked;
    }

    /// @dev Token balances
    mapping(address => Balance) private _balances;

    /// @dev Total supply
    uint256 private _totalSupply;

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
        unchecked {
            _balances[account].available += amount.toUint128();
            _totalSupply += amount.toUint128();
        }
    }

    /// @notice Locks tokens from an account
    /// @param account The address to lock tokens from
    /// @param amount The amount of tokens to lock
    function lock(address account, uint256 amount) external onlyAuthorized {
        Balance storage bal = _balances[account];
        if (bal.available < amount.toUint128()) revert InsufficientBalance();

        unchecked {
            bal.available -= amount.toUint128();
            bal.locked += amount.toUint128();
        }
    }

    /// @notice Unlocks tokens for an account
    /// @param account The address to unlock tokens for
    /// @param amount The amount of tokens to unlock
    function unlock(address account, uint256 amount) external onlyAuthorized {
        Balance storage bal = _balances[account];
        if (bal.locked < amount.toUint128()) revert InsufficientLockedBalance();

        unchecked {
            bal.locked -= amount.toUint128();
            bal.available += amount.toUint128();
        }
    }

    /// @notice Burns locked tokens from an account
    /// @param account The address to burn locked tokens from
    /// @param amount The amount of tokens to burn
    function burn(address account, uint256 amount) external onlyAuthorized {
        Balance storage bal = _balances[account];
        if (bal.locked < amount.toUint128()) revert InsufficientLockedBalance();

        unchecked {
            bal.locked -= amount.toUint128();
            _totalSupply -= amount.toUint128();
        }
    }

    /// @notice Gets both available and locked balances of an account
    /// @param account The address to query
    /// @return available The number of available tokens available
    /// @return locked The number of tokens currently locked
    function balanceOf(address account) external view returns (uint256 available, uint256 locked) {
        Balance storage bal = _balances[account];
        return (uint256(bal.available), uint256(bal.locked));
    }

    /// @notice Gets the total supply of tokens
    /// @return The total number of tokens in existence
    function totalSupply() external view returns (uint256) {
        return _totalSupply;
    }
}
