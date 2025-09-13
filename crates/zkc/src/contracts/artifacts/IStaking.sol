// SPDX-License-Identifier: MIT
pragma solidity 0.8.26;

import {IERC721} from "@openzeppelin/contracts/interfaces/IERC721.sol";

/// @title IStaking
/// @notice Interface for veZKC staking functionality
/// @dev This interface defines the core staking operations for the veZKC system
interface IStaking is IERC721 {
    error ZeroAmount();
    error UserAlreadyHasActivePosition();
    error NoActivePosition();
    error TokenDoesNotExist();
    error CannotAddToWithdrawingPosition();
    error WithdrawalAlreadyInitiated();
    error WithdrawalNotInitiated();
    error WithdrawalPeriodNotComplete();
    error NonTransferable();
    error MustUndelegateVotesFirst();
    error MustUndelegateRewardsFirst();

    /// @notice Emitted when a new stake position is created
    /// @param tokenId The ID of the newly minted veZKC NFT
    /// @param owner The address that owns the new stake position
    /// @param amount The amount of ZKC tokens staked
    event StakeCreated(uint256 indexed tokenId, address indexed owner, uint256 amount);

    /// @notice Emitted when additional tokens are added to an existing stake
    /// @param tokenId The ID of the veZKC NFT that was increased
    /// @param owner The address that owns the stake position
    /// @param addedAmount The amount of ZKC tokens added to the stake
    /// @param newTotal The new total amount of ZKC tokens in the stake
    event StakeAdded(uint256 indexed tokenId, address indexed owner, uint256 addedAmount, uint256 newTotal);

    /// @notice Emitted when a veZKC NFT is burned after unstaking is completed
    /// @param tokenId The ID of the burned veZKC NFT
    event StakeBurned(uint256 indexed tokenId);

    /// @notice Emitted when a user initiates the unstaking process
    /// @param tokenId The ID of the veZKC NFT being unstaked
    /// @param owner The address that owns the stake position
    /// @param withdrawableAt The timestamp when the unstake can be completed
    event UnstakeInitiated(uint256 indexed tokenId, address indexed owner, uint256 withdrawableAt);

    /// @notice Emitted when unstaking is completed and tokens are returned to the owner
    /// @param tokenId The ID of the veZKC NFT that was unstaked
    /// @param owner The address that received the unstaked tokens
    /// @param amount The amount of ZKC tokens that were returned
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
