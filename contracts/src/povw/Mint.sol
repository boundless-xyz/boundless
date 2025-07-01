// Copyright (c) 2025 RISC Zero, Inc.
//
// All rights reserved.

pragma solidity ^0.8.24;

import {IERC20} from "openzeppelin/contracts/token/ERC20/ERC20.sol";
import {IRiscZeroVerifier} from "risc0/IRiscZeroSetVerifier.sol";
import {Math} from "openzeppelin/contracts/utils/math/Math.sol";
import {PoVW} from "./PoVW.sol";
import {IERC20Mint} from "./IERC20Mint.sol";
import {Steel} from "risc0/steel/Steel.sol";

struct FixedPoint {
    uint256 value;
}

library FixedPointLib {
    uint8 private constant BITS = 64;

    function mulUnwrap(FixedPoint memory self, uint256 rhs) internal pure returns (uint256) {
        return Math.mulShr(self.value, rhs, BITS, Math.Rounding.Trunc);
    }
}

struct MintCalculatorUpdate {
    address workLogId;
    bytes32 initialCommit;
    bytes32 finalCommit;
}

struct MintCalculatorMint {
    address recipient;
    // Value of the mint towards the recipient, as a fraction of the epoch reward.
    // NOTE: This may be larger than 1 when aggregating rewards across multiple epochs.
    // TODO: This only works if the epoch reward is constant per epoch.
    FixedPoint value;
}

struct MintCalculatorJournal {
    MintCalculatorMint[] mints;
    MintCalculatorUpdate[] updates;
    // Address of the queried PoVW contract. Must be checked to be equal to the expected address.
    address povwContractAddress;
    Steel.Commitment steelCommit;
}

contract Mint {
    using FixedPointLib for FixedPoint;

    IRiscZeroVerifier internal immutable VERIFIER;
    IERC20Mint internal immutable TOKEN;
    PoVW internal immutable POVW;

    // TODO: Extract to a shared library along with EPOCH_LENGTH.
    // NOTE: Example value of 100 tokens per epoch, assuming 18 decimals.
    uint256 internal constant EPOCH_REWARD = 100 * 10 ** 18;

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

    constructor(IRiscZeroVerifier verifier, PoVW povw, bytes32 mintCalculatorId, IERC20Mint token) {
        VERIFIER = verifier;
        MINT_CALCULATOR_ID = mintCalculatorId;
        TOKEN = token;
        POVW = povw;
    }

    function mint(MintCalculatorJournal calldata journal, bytes calldata seal) external {
        VERIFIER.verify(seal, MINT_CALCULATOR_ID, sha256(abi.encode(journal)));
        require(Steel.validateCommitment(journal.steelCommit));
        require(journal.povwContractAddress == address(POVW));

        // Ensure the initial commit for each update is correct and update the final commit.
        for (uint256 i = 0; i < journal.updates.length; i++) {
            MintCalculatorUpdate calldata update = journal.updates[i];
            require(update.initialCommit == lastCommit[update.workLogId]);
            lastCommit[update.workLogId] = update.finalCommit;
        }

        // Issue all of the mint calls indicated in the journal.
        for (uint256 i = 0; i < journal.mints.length; i++) {
            MintCalculatorMint calldata mintData = journal.mints[i];
            uint256 mintValue = mintData.value.mulUnwrap(EPOCH_REWARD);
            TOKEN.mint(mintData.recipient, mintValue);
        }
    }
}
