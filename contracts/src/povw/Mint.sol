// Copyright (c) 2025 RISC Zero, Inc.
//
// All rights reserved.

pragma solidity ^0.8.24;

import {IRiscZeroVerifier} from "risc0/IRiscZeroSetVerifier.sol";
import {Steel} from "risc0/steel/Steel.sol";
import {IERC20} from "openzeppelin/contracts/token/ERC20/ERC20.sol";

interface IERC20Mint is IERC20 {
    /// A sender-authorized mint function as a placeholder for a real minting mechanism.
    function mint(address to, uint256 value) external;
}

struct MintCalculatorUpdate {
    address workLogId;
    bytes32 initialCommit;
    bytes32 finalCommit;
}

struct MintCalculatorMint {
    address recipient;
    uint256 value;
}

struct MintCalculatorJournal {
    MintCalculatorMint[] mints;
    MintCalculatorUpdate[] updates;
    Steel.Commitment steelCommit;
}

contract Mint {
    IRiscZeroVerifier internal immutable VERIFIER;
    IERC20Mint internal immutable TOKEN;

    // TODO: How should the mint recipient be decided? A simple answer would be mint to the work log
    // ID as an address. They are required to know the associated private key, to authorize the work
    // log updates. However, this could be a headache in that the work log key, which previously
    // has low privileges is now has custody of funds unless the owner sweeps the minted tokens into
    // a separate account on a regular basis. Other options include setting a mint recipient on this
    // contract, or having the work log owner sign a mint authorization message with the intended
    // recipient.

    /// @notice Image ID of the mint calculator guest.
    /// @dev The mint calculator ensures:
    /// * An event was logged by the PoVW contract for each log update and epoch finalization.
    ///   * Each event is counted at most once.
    ///   * Events from an unbroken chain from initialCommit to finalCommit. This constitutes an
    ///     exhaustiveness check such that the prover cannot exclude updates, and thereby deny a reward.
    /// * Mint value is calculated correctly from the PoVW totals in each included epoch.
    ///   * An event was logged by the PoVW contract for epoch finalization.
    ///   * The total work from the epoch finalization event is used in the mint calculation.
    ///   * The mint recipient is set correctly.
    bytes32 internal immutable MINT_CALCULATOR_ID;

    /// Mapping from work log ID to the most recent work log commit for which a mint has occurred.
    /// Each time a mint occurs associated with a work log, this value ratchets forward.
    mapping(address => bytes32) internal lastCommit;

    constructor(IRiscZeroVerifier verifier, bytes32 mintCalculatorId, IERC20Mint token) {
        VERIFIER = verifier;
        MINT_CALCULATOR_ID = mintCalculatorId;
        TOKEN = token;
    }

    function mint(MintCalculatorJournal calldata journal, bytes calldata seal) external {
        VERIFIER.verify(seal, MINT_CALCULATOR_ID, sha256(abi.encode(journal)));
        require(Steel.validateCommitment(journal.steelCommit));

        // Ensure the initial commit for each update is correct and update the final commit.
        for (uint256 i = 0; i < journal.updates.length; i++) {
            MintCalculatorUpdate calldata update = journal.updates[i];
            require(update.initialCommit == lastCommit[update.workLogId]);
            lastCommit[update.workLogId] = update.finalCommit;
        }

        // Issue all of the mint calls indicated in the journal.
        for (uint256 i = 0; i < journal.mints.length; i++) {
            MintCalculatorMint calldata mintData = journal.mints[i];
            TOKEN.mint(mintData.recipient, mintData.value);
        }
    }
}
