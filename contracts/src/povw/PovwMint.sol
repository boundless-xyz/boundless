// Copyright (c) 2025 RISC Zero, Inc.
//
// All rights reserved.

pragma solidity ^0.8.24;

import {IRiscZeroVerifier} from "risc0/IRiscZeroSetVerifier.sol";
import {Math} from "openzeppelin/contracts/utils/math/Math.sol";
import {PovwAccounting, EMPTY_LOG_ROOT} from "./PovwAccounting.sol";
import {IZKC, IZKCRewards} from "./IZKC.sol";
import {Steel} from "risc0/steel/Steel.sol";

struct FixedPoint {
    uint256 value;
}

library FixedPointLib {
    uint8 private constant BITS = 128;

    function mulUnwrap(FixedPoint memory self, uint256 rhs) internal pure returns (uint256) {
        return Math.mulShr(self.value, rhs, BITS, Math.Rounding.Trunc);
    }
}

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
    /// Value of the mint towards the recipient, as a fraction of the epoch reward.
    // NOTE: This may be larger than 1 when aggregating rewards across multiple epochs.
    // TODO(povw): This only works if the epoch reward is constant per epoch.
    FixedPoint value;
}

/// Journal committed by the mint calculator guest, which contains update and mint actions.
struct MintCalculatorJournal {
    /// Updates the work log commitments.
    MintCalculatorMint[] mints;
    /// Mints to issue.
    MintCalculatorUpdate[] updates;
    /// Address of the queried PovwAccounting contract. Must be checked to be equal to the expected address.
    address povwContractAddress;
    /// Address of the queried IZKCRewards contract. Must be checked to be equal to the expected address.
    address zkcRewardsAddress;
    /// A Steel commitment. Must be a valid commitment in the current chain.
    Steel.Commitment steelCommit;
}

/// PovwMint controls the minting of token rewards associated with Proof of Verifiable Work (PoVW).
///
/// This contract consumes updates produced by the mint calculator guest, mints token rewards, and
/// maintains state to ensure that any given token reward is minted at most once.
contract PovwMint {
    using FixedPointLib for FixedPoint;

    /// @dev selector 0x36ce79a0
    error InvalidSteelCommitment();
    /// @dev selector 0x82db2de2
    error IncorrectPovwAddress(address expected, address received);
    /// @dev selector 0xf4a2b615
    error IncorrectInitialUpdateCommit(bytes32 expected, bytes32 received);

    IRiscZeroVerifier internal immutable VERIFIER;
    IZKC internal immutable TOKEN;
    IZKCRewards internal immutable TOKEN_REWARDS;
    PovwAccounting internal immutable ACCOUNTING;

    // TODO(povw): Extract to a shared library along with EPOCH_LENGTH.
    // NOTE: Example value of 100 tokens per epoch, assuming 18 decimals.
    uint256 public constant EPOCH_REWARD = 100 * 10 ** 18;

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
    bytes32 internal immutable MINT_CALCULATOR_ID;

    /// @notice Mapping from work log ID to the most recent work log commit for which a mint has occurred.
    /// @notice Each time a mint occurs associated with a work log, this value ratchets forward.
    /// It ensure that any given work log update can be used in at most one mint.
    mapping(address => bytes32) internal lastCommit;

    constructor(
        IRiscZeroVerifier verifier,
        PovwAccounting povw,
        bytes32 mintCalculatorId,
        IZKC token,
        IZKCRewards tokenRewards
    ) {
        VERIFIER = verifier;
        MINT_CALCULATOR_ID = mintCalculatorId;
        ACCOUNTING = povw;
        TOKEN = token;
        TOKEN_REWARDS = tokenRewards;
    }

    /// @notice Mint tokens as a reward for verifiable work.
    function mint(bytes calldata journalBytes, bytes calldata seal) external {
        // Verify the mint is authorized by the mint calculator guest.
        VERIFIER.verify(seal, MINT_CALCULATOR_ID, sha256(journalBytes));
        MintCalculatorJournal memory journal = abi.decode(journalBytes, (MintCalculatorJournal));
        if (!Steel.validateCommitment(journal.steelCommit)) {
            revert InvalidSteelCommitment();
        }
        if (journal.povwContractAddress != address(ACCOUNTING)) {
            revert IncorrectPovwAddress({expected: address(ACCOUNTING), received: journal.povwContractAddress});
        }
        if (journal.zkcRewardsAddress != address(TOKEN_REWARDS)) {
            revert IncorrectPovwAddress({expected: address(ACCOUNTING), received: journal.povwContractAddress});
        }

        // Ensure the initial commit for each update is correct and update the final commit.
        for (uint256 i = 0; i < journal.updates.length; i++) {
            MintCalculatorUpdate memory update = journal.updates[i];

            // On the first mint for a journal, the initialCommit should be equal to the empty root.
            bytes32 expectedCommit = lastCommit[update.workLogId];
            if (expectedCommit == bytes32(0)) {
                expectedCommit = EMPTY_LOG_ROOT;
            }

            if (update.initialCommit != expectedCommit) {
                revert IncorrectInitialUpdateCommit({expected: expectedCommit, received: update.initialCommit});
            }
            lastCommit[update.workLogId] = update.updatedCommit;
        }

        // Issue all of the mint calls indicated in the journal.
        for (uint256 i = 0; i < journal.mints.length; i++) {
            MintCalculatorMint memory mintData = journal.mints[i];
            uint256 mintValue = mintData.value.mulUnwrap(EPOCH_REWARD);
            TOKEN.mintPoVWRewardsForRecipient(mintData.recipient, mintValue);
        }
    }
}
