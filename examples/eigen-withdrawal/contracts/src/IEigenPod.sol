// Copyright (c) 2024 RISC Zero, Inc.
//
// All rights reserved.

pragma solidity ^0.8.12;

interface IEigenPod {
    event NewValidator(uint40 validatorIndex, uint64 stake);
    event StakeUpdated(uint40 validatorIndex, uint64 stake);
    event ValidatorExited(uint40 validatorIndex, uint256 amount);
    event Redeemed(uint40 validatorIndex, uint64 amount);

    /// @dev Deposit function to allow the contract to receive Ether
    function topUp() external payable;

    /// @dev Drain function to allow the owner to withdraw Ether, so that we do not waste
    /// testnet funds.
    function drain() external;

    /// @dev Processes a deposit for a new validator and returns the
    /// validator index.
    function newValidator() external payable returns (uint40 validatorIndex);

    /// @dev Full withdrawal, given a journal and seal.
    ///
    /// This method will send the full withdrawal amount to the validator's withdrawal
    /// destination. The passed-in journal and seal should correspond to a zk proof attestating
    /// to the validator's credential for a full withdrawal.
    function fullWithdrawal(bytes calldata journal, bytes calldata seal) external;

    /// @dev Processes a new deposit for an existing validator.
    function updateStake(uint40 validatorIndex) external payable;

    /// @dev Partial withdrawal, given a journal and seal. This method will send the withdrawal amount
    /// to the validator's withdrawal destination. The passed-in journal and seal should correspond to a zk proof
    /// attesting to the validator's credential for a partial withdrawal.
    function partialWithdrawal(bytes calldata journal, bytes calldata seal) external;

    /// @dev Returns the stake of a validator in Gwei.
    function stakeBalanceGwei(uint40 validatorIndex) external view returns (uint64);

    /// @dev Returns the pubkey hash of a validator.
    function pubkeyHash(uint40 validatorIndex) external view returns (bytes32);

    /// @dev Returns the estimated yield at the current block of a validator in Gwei.
    function estimateYield(uint40 validatorIndex) external view returns (uint64);
}
