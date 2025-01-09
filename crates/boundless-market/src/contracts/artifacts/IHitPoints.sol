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

pragma solidity ^0.8.24;

/// @title IHitPoints - Interface for the hit points system
/// @notice Defines the interface for a non-transferrable token with locking mechanism
interface IHitPoints {
    /// @dev Thrown when a caller is not authorized for an operation
    error UnauthorizedCaller();
    /// @dev Thrown when trying to use more tokens than available
    error InsufficientBalance();
    /// @dev Thrown when trying to use more locked tokens than available
    error InsufficientLockedBalance();

    /// @notice Adds a new address to the authorized list
    /// @param account The address to authorize
    function authorize(address account) external;

    /// @notice Removes an address from the authorized list
    /// @param account The address to remove authorization from
    function deauthorize(address account) external;

    /// @notice Creates new tokens and assigns them to an account
    /// @param account The address that will receive the minted tokens
    /// @param amount The amount of tokens to mint
    function mint(address account, uint256 amount) external;

    /// @notice Locks tokens from an account
    /// @param account The address to lock tokens from
    /// @param amount The amount of tokens to lock
    function lock(address account, uint256 amount) external;

    /// @notice Unlocks tokens for an account
    /// @param account The address to unlock tokens for
    /// @param amount The amount of tokens to unlock
    function unlock(address account, uint256 amount) external;

    /// @notice Burns locked tokens from an account
    /// @param account The address to burn locked tokens from
    /// @param amount The amount of tokens to burn
    function burn(address account, uint256 amount) external;

    /// @notice Gets both available and locked balances of an account
    /// @param account The address to query
    /// @return available The number of available tokens available
    /// @return locked The number of tokens currently locked
    function balanceOf(address account) external view returns (uint256 available, uint256 locked);

    /// @notice Gets the total supply of tokens
    /// @return The total number of tokens in existence
    function totalSupply() external view returns (uint256);
}
