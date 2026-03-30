// Copyright 2026 Boundless Foundation, Inc.
//
// Use of this source code is governed by the Business Source License
// as found in the LICENSE-BSL file.
// SPDX-License-Identifier: BUSL-1.1

pragma solidity ^0.8.26;

import {IRiscZeroVerifier} from "risc0/IRiscZeroSetVerifier.sol";
import {SafeCast} from "@openzeppelin/contracts/utils/math/SafeCast.sol";
import {EIP712Upgradeable} from "@openzeppelin/contracts-upgradeable/utils/cryptography/EIP712Upgradeable.sol";
import {OwnableUpgradeable} from "@openzeppelin/contracts-upgradeable/access/OwnableUpgradeable.sol";
import {UUPSUpgradeable} from "@openzeppelin/contracts-upgradeable/proxy/utils/UUPSUpgradeable.sol";
import {Initializable} from "@openzeppelin/contracts-upgradeable/proxy/utils/Initializable.sol";
import {IZKC} from "zkc/interfaces/IZKC.sol";
import {IPovwAccounting, WorkLogUpdate, Journal, PendingEpoch} from "./IPovwAccounting.sol";

bytes32 constant EMPTY_LOG_ROOT = hex"b26927f749929e8484785e36e7ec93d5eeae4b58182f76f1e760263ab67f540c";

// Storage version of PendingEpoch, which fits in one slot.
// NOTE: Assumes that the epoch number will never exceed 64 bits
struct PendingEpochStorage {
    uint96 totalWork;
    uint64 number;
}

contract PovwAccounting is IPovwAccounting, Initializable, EIP712Upgradeable, OwnableUpgradeable, UUPSUpgradeable {
    using SafeCast for uint256;

    /// @dev The version of the contract, with respect to upgrades.
    uint64 public constant VERSION = 1;

    /// @custom:oz-upgrades-unsafe-allow state-variable-immutable
    IRiscZeroVerifier public immutable VERIFIER;

    /// Image ID of the work log updater guest. The log updater ensures:
    /// @dev The log updater ensures:
    ///
    /// * Update is signed by the ECDSA key associated with the log ID.
    /// * State transition from initial to updated root is append-only.
    /// * The update value is equal to the sum of work associated with new proofs.
    ///
    /// The log updater achieves some of these properties by verifying a proof from the log builder.
    /// @custom:oz-upgrades-unsafe-allow state-variable-immutable
    bytes32 public immutable LOG_UPDATER_ID;

    /// @custom:oz-upgrades-unsafe-allow state-variable-immutable
    IZKC public immutable TOKEN;

    mapping(address => bytes32) internal workLogCommits;

    PendingEpochStorage internal _pendingEpoch;

    // NOTE: When updating this constructor, crates/povw/build.rs must be updated as well.
    /// @custom:oz-upgrades-unsafe-allow constructor
    constructor(IRiscZeroVerifier verifier, IZKC token, bytes32 logUpdaterId) {
        require(address(verifier) != address(0), "verifier cannot be zero");
        require(address(token) != address(0), "token cannot be zero");
        require(logUpdaterId != bytes32(0), "logUpdaterId cannot be zero");
        VERIFIER = verifier;
        TOKEN = token;
        LOG_UPDATER_ID = logUpdaterId;

        _disableInitializers();
    }

    function initialize(address initialOwner) external initializer {
        __Ownable_init(initialOwner);
        __UUPSUpgradeable_init();
        __EIP712_init("PovwAccounting", "1");

        _pendingEpoch = PendingEpochStorage({number: TOKEN.getCurrentEpoch().toUint64(), totalWork: 0});
    }

    function _authorizeUpgrade(address newImplementation) internal override onlyOwner {}

    /// @inheritdoc IPovwAccounting
    function pendingEpoch() external view returns (PendingEpoch memory) {
        return PendingEpoch({totalWork: _pendingEpoch.totalWork, number: _pendingEpoch.number});
    }

    /// @inheritdoc IPovwAccounting
    function finalizeEpoch() public {
        uint64 newEpoch = TOKEN.getCurrentEpoch().toUint64();
        require(_pendingEpoch.number < newEpoch, "pending epoch has not ended");

        _finalizePendingEpoch(newEpoch);
    }

    /// End the pending epoch and start the new epoch. This function should
    /// only be called after checking that the pending epoch has ended.
    function _finalizePendingEpoch(uint64 newEpoch) internal {
        // Emit the epoch finalized event, accessed with Steel to construct the mint authorization.
        emit EpochFinalized(_pendingEpoch.number, _pendingEpoch.totalWork);

        // NOTE: This may cause the epoch number to increase by more than 1, if no updates occurred in
        // an interim epoch. Any interim epoch that was skipped will have no work associated with it.
        _pendingEpoch = PendingEpochStorage({number: newEpoch, totalWork: 0});
    }

    /// @inheritdoc IPovwAccounting
    function updateWorkLog(
        address workLogId,
        bytes32 updatedCommit,
        uint64 updateValue,
        address valueRecipient,
        bytes calldata seal
    ) public {
        uint64 currentEpoch = TOKEN.getCurrentEpoch().toUint64();
        if (_pendingEpoch.number < currentEpoch) {
            _finalizePendingEpoch(currentEpoch);
        }

        // Fetch the initial commit value, substituting with the precomputed empty root if new.
        bytes32 initialCommit = workLogCommit(workLogId);

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

        workLogCommits[workLogId] = updatedCommit;
        _pendingEpoch.totalWork += uint96(updateValue);

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
    function workLogCommit(address workLogId) public view returns (bytes32) {
        bytes32 commit = workLogCommits[workLogId];
        if (commit == bytes32(0)) {
            return EMPTY_LOG_ROOT;
        }
        return commit;
    }
}
