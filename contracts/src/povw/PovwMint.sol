// Copyright (c) 2025 RISC Zero, Inc.
//
// All rights reserved.

pragma solidity ^0.8.24;

import {IRiscZeroVerifier} from "risc0/IRiscZeroSetVerifier.sol";
import {Math} from "openzeppelin/contracts/utils/math/Math.sol";
import {PovwAccounting, EMPTY_LOG_ROOT} from "./PovwAccounting.sol";
import {IZKC, IZKCRewards} from "./IZKC.sol";
import {IPovwMint, MintCalculatorUpdate, MintCalculatorMint, MintCalculatorJournal} from "./IPovwMint.sol";
import {Steel} from "steel/Steel.sol";

/// PovwMint controls the minting of token rewards associated with Proof of Verifiable Work (PoVW).
///
/// This contract consumes updates produced by the mint calculator guest, mints token rewards, and
/// maintains state to ensure that any given token reward is minted at most once.
contract PovwMint is IPovwMint {
    IRiscZeroVerifier public immutable VERIFIER;
    IZKC public immutable TOKEN;
    IZKCRewards public immutable TOKEN_REWARDS;
    PovwAccounting public immutable ACCOUNTING;

    /// @notice Image ID of the mint calculator guest.
    /// @dev The mint calculator ensures:
    /// * An event was logged by the PoVW accounting contract for each log update and epoch finalization.
    ///   * Each event is counted at most once.
    ///   * Events form an unbroken chain from initialCommit to updatedCommit. This constitutes an
    ///     exhaustiveness check such that the prover cannot exclude updates, and thereby deny a reward.
    /// * Mint value is calculated correctly from the PoVW totals in each included epoch.
    ///   * An event was logged by the PoVW accounting contract for epoch finalization.
    ///   * The total work from the epoch finalization event is used in the mint calculation.
    ///   * The mint recipient is set correctly.
    bytes32 public immutable MINT_CALCULATOR_ID;

    /// @notice Mapping from work log ID to the most recent work log commit for which a mint has occurred.
    /// @notice Each time a mint occurs associated with a work log, this value ratchets forward.
    /// It ensure that any given work log update can be used in at most one mint.
    mapping(address => bytes32) public workLogCommits;

    // NOTE: When updating this constructor, crates/guest/povw/build.rs must be updated as well.
    constructor(
        IRiscZeroVerifier verifier,
        PovwAccounting accounting,
        bytes32 mintCalculatorId,
        IZKC token,
        IZKCRewards tokenRewards
    ) {
        require(address(verifier) != address(0), "verifier cannot be zero");
        require(address(accounting) != address(0), "accounting cannot be zero");
        require(address(tokenRewards) != address(0), "tokenRewards cannot be zero");
        require(address(token) != address(0), "token cannot be zero");
        require(mintCalculatorId != bytes32(0), "mintCalculatorId cannot be zero");

        VERIFIER = verifier;
        ACCOUNTING = accounting;
        TOKEN = token;
        TOKEN_REWARDS = tokenRewards;
        MINT_CALCULATOR_ID = mintCalculatorId;
    }

    /// @inheritdoc IPovwMint
    function mint(bytes calldata journalBytes, bytes calldata seal) external {
        // Verify the mint is authorized by the mint calculator guest.
        VERIFIER.verify(seal, MINT_CALCULATOR_ID, sha256(journalBytes));
        MintCalculatorJournal memory journal = abi.decode(journalBytes, (MintCalculatorJournal));
        if (!Steel.validateCommitment(journal.steelCommit)) {
            revert InvalidSteelCommitment();
        }
        if (journal.povwAccountingAddress != address(ACCOUNTING)) {
            revert IncorrectSteelContractAddress({
                expected: address(ACCOUNTING),
                received: journal.povwAccountingAddress
            });
        }
        if (journal.zkcAddress != address(TOKEN)) {
            revert IncorrectSteelContractAddress({expected: address(TOKEN), received: journal.zkcAddress});
        }
        if (journal.zkcRewardsAddress != address(TOKEN_REWARDS)) {
            revert IncorrectSteelContractAddress({expected: address(TOKEN_REWARDS), received: journal.zkcRewardsAddress});
        }

        // Ensure the initial commit for each update is correct and update the final commit.
        for (uint256 i = 0; i < journal.updates.length; i++) {
            MintCalculatorUpdate memory update = journal.updates[i];

            // On the first mint for a journal, the initialCommit should be equal to the empty root.
            bytes32 expectedCommit = workLogCommits[update.workLogId];
            if (expectedCommit == bytes32(0)) {
                expectedCommit = EMPTY_LOG_ROOT;
            }

            if (update.initialCommit != expectedCommit) {
                revert IncorrectInitialUpdateCommit({expected: expectedCommit, received: update.initialCommit});
            }
            workLogCommits[update.workLogId] = update.updatedCommit;
        }

        // Issue all of the mint calls indicated in the journal.
        for (uint256 i = 0; i < journal.mints.length; i++) {
            MintCalculatorMint memory mintData = journal.mints[i];
            TOKEN.mintPoVWRewardsForRecipient(mintData.recipient, mintData.value);
        }
    }

    /// @inheritdoc IPovwMint
    function workLogCommit(address workLogId) public view returns (bytes32) {
        bytes32 commit = workLogCommits[workLogId];
        if (commit == bytes32(0)) {
            return EMPTY_LOG_ROOT;
        }
        return commit;
    }
}
