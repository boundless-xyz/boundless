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
    /// @dev Thrown when trying to transfer tokens from/to an unauthorized address
    error UnauthorizedTransfer();
    /// @dev Thrown when balance exceeds uint96 max
    error BalanceExceedsLimit(address account, uint256 currentBalance, uint256 addedAmount);

    /// @notice Grants the MINTER role to an account
    /// @dev This role is used to allow minting new tokens
    /// @param account The address that will receive the minter role
    function grantMinterRole(address account) external;

    /// @notice Revokes the MINTER role from an account
    /// @param account The address that will lose the minter role
    function revokeMinterRole(address account) external;

    /// @notice Grants the AUTHORIZED_TRANSFER role to an account
    /// @dev This role is used to allow transfers from/to an address
    /// @param account The address that will receive the authorized transfer role
    function grantAuthorizedTransferRole(address account) external;

    /// @notice Revokes the AUTHORIZED_TRANSFER role from an account
    /// @param account The address that will lose the authorized transfer role
    function revokeAuthorizedTransferRole(address account) external;

    /// @notice Creates new tokens and assigns them to an account
    /// @param account The address that will receive the minted tokens
    /// @param value The `value` amount of tokens to mint
    function mint(address account, uint256 value) external;
}
