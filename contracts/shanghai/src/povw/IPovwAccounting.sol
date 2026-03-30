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

/// An update to a work log.
struct WorkLogUpdate {
    /// The log ID associated with this update. This log ID is interpreted as an address for the
    /// purpose of verifying a signature to authorize the update.
    address workLogId;
    /// Initial log commitment from which this update is calculated.
    /// @dev This commits to all the PoVW nonces consumed prior to this update.
    bytes32 initialCommit;
    /// Updated log commitment after the update is applied.
    /// @dev This commits to all the PoVW nonces consumed after this update.
    bytes32 updatedCommit;
    /// Work value verified in this update.
    /// @dev This value will be used by the mint calculator to allocate rewards.
    uint64 updateValue;
    /// Recipient of any rewards associated with this update, authorized by the hold of the private
    /// key associated with the work log ID.
    address valueRecipient;
}

/// Journal committed to by the log updater guest.
struct Journal {
    WorkLogUpdate update;
    /// EIP712 domain digest. The verifying contract must validate this to be equal to it own
    /// expected EIP712 domain digest.
    bytes32 eip712Domain;
}

/// The currently pending epoch, which is still active.
struct PendingEpoch {
    /// @notice Verifiable work value that has been submitted in this epoch so far.
    uint96 totalWork;
    /// @notice The pending epoch number.
    /// @dev This may not be the current epoch number in the case that the epoch deadline has passed,
    /// but the finalizeEpoch method has not been called.
    uint256 number;
}

interface IPovwAccounting {
    /// @notice Event emitted during the finalization of an epoch.
    /// @dev This event is emitted in some block after the end of the epoch, when the finalizeEpoch
    ///      function is called. Note that this is no later than the first time that updateWorkLog
    ///      is called after the pending epoch has ended.
    /// @param epoch The number of the epoch that is being finalized.
    /// @param totalWork The total value of the work submitted by provers during this epoch.
    event EpochFinalized(uint256 indexed epoch, uint256 totalWork);

    /// @notice Event emitted when when a work log update is processed.
    /// @param workLogId The work log identifier, which also serves as an authentication public key.
    /// @param epochNumber The number of the epoch in which the update is processed.
    ///        The value of the update will be weighted against the total work completed in this epoch.
    /// @param initialCommit The initial work log commitment for the update.
    /// @param updatedCommit The updated work log commitment after the update has been processed.
    /// @param updateValue Value of the work in this update.
    /// @param valueRecipient The recipient of any rewards associated with this update.
    event WorkLogUpdated(
        address indexed workLogId,
        uint256 epochNumber,
        bytes32 initialCommit,
        bytes32 updatedCommit,
        uint256 updateValue,
        address valueRecipient
    );

    /// Return the number and total work (so far) of the pending epoch.
    function pendingEpoch() external view returns (PendingEpoch memory);

    /// Finalize the pending epoch, logging the finalized epoch number and total work.
    function finalizeEpoch() external;

    /// @notice Update a work log and log an event with the associated update value.
    /// @dev The stored work log root is updated, preventing the same nonce from being counted twice.
    /// Work reported in this update will be assigned to the current epoch. A receipt from the work
    /// log updater is used to ensure the integrity of the update.
    ///
    /// If an epoch is pending finalization, finalization occurs atomically with this call.
    function updateWorkLog(
        address workLogId,
        bytes32 updatedCommit,
        uint64 updateValue,
        address valueRecipient,
        bytes calldata seal
    ) external;

    /// @notice Get the current work log commitment for the given work log.
    /// @dev This commits to the consumed nonces that have been included in a log update.
    function workLogCommit(address workLogId) external view returns (bytes32);
}
