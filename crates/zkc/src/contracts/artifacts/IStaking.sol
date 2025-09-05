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

// TODO(povw) Use IStaking from the zkc repo directly.

pragma solidity ^0.8.20;

import {IERC721} from "@openzeppelin/contracts/interfaces/IERC721.sol";

/// @title IStaking
/// @notice Interface for veZKC staking functionality
/// @dev This interface defines the core staking operations for the veZKC system
interface IStaking is IERC721 {
    event StakeCreated(uint256 indexed tokenId, address indexed owner, uint256 amount);
    event StakeAdded(uint256 indexed tokenId, address indexed owner, uint256 addedAmount, uint256 newTotal);
    event StakeBurned(uint256 indexed tokenId);
    event UnstakeInitiated(uint256 indexed tokenId, address indexed owner, uint256 withdrawableAt);
    event UnstakeCompleted(uint256 indexed tokenId, address indexed owner, uint256 amount);

    /// @notice Stake ZKC tokens to mint veZKC NFT
    /// @param amount Amount of ZKC to stake
    /// @return tokenId The minted veZKC NFT token ID
    function stake(uint256 amount) external returns (uint256 tokenId);

    /// @notice Stake ZKC tokens using permit to avoid pre-approval
    /// @param amount Amount of ZKC to stake
    /// @param permitDeadline Permit deadline
    /// @param v Permit signature v
    /// @param r Permit signature r
    /// @param s Permit signature s
    /// @return tokenId The minted veZKC NFT token ID
    function stakeWithPermit(uint256 amount, uint256 permitDeadline, uint8 v, bytes32 r, bytes32 s)
        external
        returns (uint256 tokenId);

    /// @notice Add stake to your own active position
    /// @param amount Amount of ZKC to add
    function addToStake(uint256 amount) external;

    /// @notice Add stake to your own active position using permit
    /// @param amount Amount of ZKC to add
    /// @param permitDeadline Permit deadline
    /// @param v Permit signature v
    /// @param r Permit signature r
    /// @param s Permit signature s
    function addToStakeWithPermit(uint256 amount, uint256 permitDeadline, uint8 v, bytes32 r, bytes32 s) external;

    /// @notice Add stake to any user's position by token ID (donation)
    /// @param tokenId Token ID to add stake to
    /// @param amount Amount of ZKC to add
    function addToStakeByTokenId(uint256 tokenId, uint256 amount) external;

    /// @notice Add stake to any user's position by token ID using permit
    /// @param tokenId Token ID to add stake to
    /// @param amount Amount of ZKC to add
    /// @param permitDeadline Permit deadline
    /// @param v Permit signature v
    /// @param r Permit signature r
    /// @param s Permit signature s
    function addToStakeWithPermitByTokenId(
        uint256 tokenId,
        uint256 amount,
        uint256 permitDeadline,
        uint8 v,
        bytes32 r,
        bytes32 s
    ) external;

    /// @notice Initiate unstaking process (30-day withdrawal period)
    function initiateUnstake() external;

    /// @notice Complete unstaking after withdrawal period
    function completeUnstake() external;

    /// @notice Get staked amount and withdrawal completion time for an account
    /// @param account Account to query
    /// @return amount Staked amount
    /// @return withdrawableAt Timestamp when withdrawal can be completed (0 if not withdrawing)
    function getStakedAmountAndWithdrawalTime(address account)
        external
        view
        returns (uint256 amount, uint256 withdrawableAt);

    /// @notice Get active token ID for a user
    /// @param user User to query
    /// @return tokenId Active token ID (0 if none)
    function getActiveTokenId(address user) external view returns (uint256 tokenId);
}
