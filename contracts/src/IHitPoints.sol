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

/// @title IHitPoints ERC20
/// @notice Interface of a restricted transfer token using ERC20
interface IHitPoints {
    /// @dev Thrown when a caller is not authorized for an operation
    error Unauthorized();
    /// @dev Thrown when trying to transfer tokens from/to an unauthorized address
    error UnauthorizedTransfer();
    /// @dev Thrown when balance exceeds uint96 max
    error BalanceExceedsLimit(address account, uint256 currentBalance, uint256 addedAmount);

    /// @notice Adds a new address to the authorized list
    /// @param account The address to authorize
    function authorize(address account) external;

    /// @notice Removes an address from the authorized list
    /// @param account The address to remove authorization from
    function deauthorize(address account) external;

    /// @notice Creates new tokens and assigns them to an account
    /// @param account The address that will receive the minted tokens
    /// @param value The `value` amount of tokens to mint
    function mint(address account, uint256 value) external;
}
