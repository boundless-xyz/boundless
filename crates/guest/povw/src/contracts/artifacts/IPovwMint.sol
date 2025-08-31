// Copyright (c) 2025 RISC Zero, Inc.
//
// All rights reserved.

pragma solidity ^0.8.24;

import {Steel} from "steel/Steel.sol";

/// An update to the commitment for the processing of a work log.
struct MintCalculatorUpdate {
    /// Work log ID associated that is updated.
    address workLogId;
    /// The initial value of the log commitment to which this update is based on.
    bytes32 initialCommit;
    /// The value of the log commitment after this update is applied.
    bytes32 updatedCommit;
}

/// A mint action authorized by the mint calculator.
struct MintCalculatorMint {
    /// Address of the recipient for the mint.
    address recipient;
    /// Value of the rewards to credit towards the recipient.
    uint256 value;
}

/// Journal committed by the mint calculator guest, which contains update and mint actions.
struct MintCalculatorJournal {
    /// Updates the work log commitments.
    MintCalculatorMint[] mints;
    /// Mints to issue.
    MintCalculatorUpdate[] updates;
    /// Address of the queried PovwAccounting contract. Must be checked to be equal to the expected address.
    address povwAccountingAddress;
    /// Address of the queried IZKCRewards contract. Must be checked to be equal to the expected address.
    address zkcRewardsAddress;
    /// Address of the queried IZKC contract. Must be checked to be equal to the expected address.
    address zkcAddress;
    /// A Steel commitment. Must be a valid commitment in the current chain.
    Steel.Commitment steelCommit;
}

interface IPovwMint {
    /// @dev selector 0x36ce79a0
    error InvalidSteelCommitment();
    /// @dev selector 0x98d6328f
    error IncorrectSteelContractAddress(address expected, address received);
    /// @dev selector 0xf4a2b615
    error IncorrectInitialUpdateCommit(bytes32 expected, bytes32 received);

    /// @notice Mint tokens as a reward for verifiable work.
    function mint(bytes calldata journalBytes, bytes calldata seal) external;
}