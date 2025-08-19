// Copyright (c) 2025 RISC Zero, Inc.
//
// All rights reserved.

pragma solidity ^0.8.24;

import {IRiscZeroVerifier} from "risc0/IRiscZeroSetVerifier.sol";
import {EIP712} from "@openzeppelin/contracts/utils/cryptography/EIP712.sol";
import {IZKC} from "./IZKC.sol";

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

// TODO(povw): If we can guarentee that the epoch number will never be greater than uint160, this
// could be compressed into one slot.
struct PendingEpoch {
    uint96 totalWork;
    uint256 number;
}

bytes32 constant EMPTY_LOG_ROOT = hex"b26927f749929e8484785e36e7ec93d5eeae4b58182f76f1e760263ab67f540c";

contract PovwAccounting is EIP712 {
    IRiscZeroVerifier public immutable VERIFIER;

    /// Image ID of the work log updater guest. The log updater ensures:
    /// @dev The log updater ensures:
    ///
    /// * Update is signed by the ECDSA key associated with the log ID.
    /// * State transition from initial to updated root is append-only.
    /// * The update value is equal to the sum of work associated with new proofs.
    ///
    /// The log updater achieves some of these properties by verifying a proof from the log builder.
    bytes32 public immutable LOG_UPDATER_ID;

    IZKC internal immutable TOKEN;
    uint256 public constant EPOCH_LENGTH = 7 days;

    mapping(address => bytes32) internal workLogRoots;

    PendingEpoch public pendingEpoch;

    event EpochFinalized(uint256 indexed epoch, uint256 totalWork);
    // TODO(povw): Compress the data in this event? epochNumber is a simple view function of the
    // block timestamp and is 32 bits. Update value is 64 bits. At least 32 bytes could be saved
    // here with compression.
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

    constructor(IRiscZeroVerifier verifier, IZKC token, bytes32 logUpdaterId) EIP712("PovwAccounting", "1") {
        VERIFIER = verifier;
        TOKEN = token;
        LOG_UPDATER_ID = logUpdaterId;

        pendingEpoch = PendingEpoch({number: TOKEN.getCurrentEpoch(), totalWork: 0});
    }

    /// Finalize the pending epoch, logging the finalized epoch number and total work.
    function finalizeEpoch() public {
        uint256 newEpoch = TOKEN.getCurrentEpoch();
        require(pendingEpoch.number < newEpoch, "pending epoch has not ended");

        _finalizePendingEpoch(newEpoch);
    }

    /// End the pending epoch and start the new epoch. This function should
    /// only be called after checking that the pending epoch has ended.
    function _finalizePendingEpoch(uint256 newEpoch) internal {
        // Emit the epoch finalized event, accessed with Steel to construct the mint authorization.
        emit EpochFinalized(uint256(pendingEpoch.number), uint256(pendingEpoch.totalWork));

        // NOTE: This may cause the epoch number to increase by more than 1, if no updates occurred in
        // an interim epoch. Any interim epoch that was skipped will have no work associated with it.
        pendingEpoch = PendingEpoch({number: newEpoch, totalWork: 0});
    }

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
    ) public {
        uint256 currentEpoch = TOKEN.getCurrentEpoch();
        if (pendingEpoch.number < currentEpoch) {
            _finalizePendingEpoch(currentEpoch);
        }

        // Fetch the initial commit value, substituting with the precomputed empty root if new.
        bytes32 initialCommit = workLogRoots[workLogId];
        if (initialCommit == bytes32(0)) {
            initialCommit = EMPTY_LOG_ROOT;
        }

        // Verify the receipt from the work log builder, binding the initial root as the currently
        // stored value.
        WorkLogUpdate memory update = WorkLogUpdate({
            workLogId: workLogId,
            initialCommit: initialCommit,
            updatedCommit: updatedCommit,
            updateValue: updateValue,
            valueRecipient: valueRecipient
        });
        Journal memory journal = Journal({update: update, eip712Domain: _domainSeparatorV4()});
        VERIFIER.verify(seal, LOG_UPDATER_ID, sha256(abi.encode(journal)));

        workLogRoots[workLogId] = updatedCommit;
        pendingEpoch.totalWork += updateValue;

        // Emit the update event, accessed with Steel to construct the mint authorization.
        // Note that there is no restriction on multiple updates in the same epoch. Posting more than
        // one update in an epoch.
        emit WorkLogUpdated(
            workLogId,
            currentEpoch,
            update.initialCommit,
            update.updatedCommit,
            uint256(updateValue),
            update.valueRecipient
        );
    }
}
