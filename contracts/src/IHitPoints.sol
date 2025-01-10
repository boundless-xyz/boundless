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
    error InsufficientBalance(address account);

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

    /// @notice Burns locked tokens from an account
    /// @param account The address to burn locked tokens from
    /// @param amount The amount of tokens to burn
    function burn(address account, uint256 amount) external;

    /// @notice Gets the balance of an account
    /// @param account The address to query
    /// @return balance The number of tokens available
    function balanceOf(address account) external view returns (uint256 balance);
}
