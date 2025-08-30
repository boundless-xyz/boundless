// Copyright (c) 2025 RISC Zero, Inc.
//
// All rights reserved.

pragma solidity ^0.8.24;

import {IRiscZeroVerifier} from "risc0/IRiscZeroSetVerifier.sol";
import {EIP712} from "@openzeppelin/contracts/utils/cryptography/EIP712.sol";
import {IZKC} from "./IZKC.sol";
import {IPovwAccounting, WorkLogUpdate, Journal} from "./IPovwAccounting.sol";

// TODO(povw): If we can guarentee that the epoch number will never be greater than uint160, this
// could be compressed into one slot.
struct PendingEpoch {
    uint96 totalWork;
    uint256 number;
}

bytes32 constant EMPTY_LOG_ROOT = hex"b26927f749929e8484785e36e7ec93d5eeae4b58182f76f1e760263ab67f540c";

contract PovwAccounting is IPovwAccounting, EIP712 {
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

    IZKC public immutable TOKEN;

    mapping(address => bytes32) internal workLogRoots;

    PendingEpoch public pendingEpoch;

    constructor(IRiscZeroVerifier verifier, IZKC token, bytes32 logUpdaterId) EIP712("PovwAccounting", "1") {
        VERIFIER = verifier;
        TOKEN = token;
        LOG_UPDATER_ID = logUpdaterId;

        pendingEpoch = PendingEpoch({number: TOKEN.getCurrentEpoch(), totalWork: 0});
    }

    /// @inheritdoc IPovwAccounting
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

    /// @inheritdoc IPovwAccounting
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

    /// @inheritdoc IPovwAccounting
    function getWorkLogCommit(address workLogId) external view returns (bytes32) {
        return workLogRoots[workLogId];
    }
}
