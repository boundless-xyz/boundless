// Copyright 2026 Boundless Foundation, Inc.
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

pragma solidity ^0.8.26;

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

/// PovwMint controls the minting of token rewards associated with Proof of Verifiable Work (PoVW).
///
/// This contract consumes updates produced by the mint calculator guest, mints token rewards, and
/// maintains state to ensure that any given token reward is minted at most once.
interface IPovwMint {
    /// @dev selector 0x36ce79a0
    error InvalidSteelCommitment();
    /// @dev selector 0x98d6328f
    error IncorrectSteelContractAddress(address expected, address received);
    /// @dev selector 0xf4a2b615
    error IncorrectInitialUpdateCommit(bytes32 expected, bytes32 received);

    /// @notice Mint tokens as a reward for verifiable work.
    function mint(bytes calldata journalBytes, bytes calldata seal) external;

    /// @notice Get the current work log commitment for the given work log.
    /// @dev This commits to the consumed nonces for all updates that have been included in a mint operation.
    function workLogCommit(address workLogId) external view returns (bytes32);
}
