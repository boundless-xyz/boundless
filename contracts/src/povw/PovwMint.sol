// Copyright 2026 Boundless Foundation, Inc.
//
// Use of this source code is governed by the Business Source License
// as found in the LICENSE-BSL file.
// SPDX-License-Identifier: BUSL-1.1

pragma solidity ^0.8.26;

import {IRiscZeroVerifier} from "risc0/IRiscZeroSetVerifier.sol";
import {OwnableUpgradeable} from "@openzeppelin/contracts-upgradeable/access/OwnableUpgradeable.sol";
import {UUPSUpgradeable} from "@openzeppelin/contracts-upgradeable/proxy/utils/UUPSUpgradeable.sol";
import {Initializable} from "@openzeppelin/contracts-upgradeable/proxy/utils/Initializable.sol";
import {PovwAccounting, EMPTY_LOG_ROOT} from "./PovwAccounting.sol";
import {IZKC} from "zkc/interfaces/IZKC.sol";
import {IRewards as IZKCRewards} from "zkc/interfaces/IRewards.sol";
import {
    IPovwMint,
    MintCalculatorUpdate,
    MintCalculatorMint,
    MintCalculatorJournal,
    MarketContribution,
    MarketContributionJournal
} from "./IPovwMint.sol";
import {Steel} from "steel/Steel.sol";

/// PovwMint controls the minting of token rewards associated with Proof of Verifiable Work (PoVW).
///
/// This contract consumes updates produced by the mint calculator guest, mints token rewards, and
/// maintains state to ensure that any given token reward is minted at most once.
///
/// V2 adds two-pool market-aligned rewards: Pool 1 (mining baseline) and Pool 2 (market participation).
contract PovwMint is IPovwMint, Initializable, OwnableUpgradeable, UUPSUpgradeable {
    /// @dev The version of the contract, with respect to upgrades.
    uint64 public constant VERSION = 2;

    /// @custom:oz-upgrades-unsafe-allow state-variable-immutable
    IRiscZeroVerifier public immutable VERIFIER;
    /// @custom:oz-upgrades-unsafe-allow state-variable-immutable
    IZKC public immutable TOKEN;
    /// @custom:oz-upgrades-unsafe-allow state-variable-immutable
    IZKCRewards public immutable TOKEN_REWARDS;
    /// @custom:oz-upgrades-unsafe-allow state-variable-immutable
    PovwAccounting public immutable ACCOUNTING;

    /// @notice Image ID of the mint calculator guest.
    /// @custom:oz-upgrades-unsafe-allow state-variable-immutable
    bytes32 public immutable MINT_CALCULATOR_ID;

    /// @notice Image ID of the market contribution calculator guest.
    /// @custom:oz-upgrades-unsafe-allow state-variable-immutable
    bytes32 public immutable MARKET_CONTRIBUTION_CALCULATOR_ID;

    /// @notice Address of the BoundlessMarket contract.
    /// @custom:oz-upgrades-unsafe-allow state-variable-immutable
    address public immutable MARKET_ADDRESS;

    /// @notice Mapping from work log ID to the most recent work log commit for which a mint has occurred.
    /// @notice Each time a mint occurs associated with a work log, this value ratchets forward.
    /// It ensure that any given work log update can be used in at most one mint.
    mapping(address => bytes32) public workLogCommits;

    /// @notice Pool 1 fraction in basis points. e.g. 3000 = 30% mining, 70% market.
    uint256 public pool1FractionBps;

    /// @notice Replay guard for journals (shared by mint and commitEpoch).
    mapping(bytes32 => bool) public commitProcessed;

    // NOTE: When updating this constructor, crates/povw/build.rs must be updated as well.
    /// @custom:oz-upgrades-unsafe-allow constructor
    constructor(
        IRiscZeroVerifier verifier,
        PovwAccounting accounting,
        bytes32 mintCalculatorId,
        bytes32 marketContributionCalculatorId,
        address marketAddress,
        IZKC token,
        IZKCRewards tokenRewards
    ) {
        require(address(verifier) != address(0), "verifier cannot be zero");
        require(address(accounting) != address(0), "accounting cannot be zero");
        require(address(tokenRewards) != address(0), "tokenRewards cannot be zero");
        require(address(token) != address(0), "token cannot be zero");
        require(mintCalculatorId != bytes32(0), "mintCalculatorId cannot be zero");
        require(marketContributionCalculatorId != bytes32(0), "marketContributionCalculatorId cannot be zero");
        require(marketAddress != address(0), "marketAddress cannot be zero");

        VERIFIER = verifier;
        ACCOUNTING = accounting;
        TOKEN = token;
        TOKEN_REWARDS = tokenRewards;
        MINT_CALCULATOR_ID = mintCalculatorId;
        MARKET_CONTRIBUTION_CALCULATOR_ID = marketContributionCalculatorId;
        MARKET_ADDRESS = marketAddress;

        _disableInitializers();
    }

    function initialize(address initialOwner) external initializer {
        __Ownable_init(initialOwner);
        __UUPSUpgradeable_init();
    }

    /// @notice Initialize V2 state. Called via upgradeToAndCall.
    function initializeV2(uint256 _pool1FractionBps) external reinitializer(2) {
        if (_pool1FractionBps > 10000) {
            revert InvalidPool1FractionBps(_pool1FractionBps);
        }
        pool1FractionBps = _pool1FractionBps;
    }

    function _authorizeUpgrade(address newImplementation) internal override onlyOwner {}

    /// @dev Verify the mint calculator proof, advance the commit ratchet, and mark as processed.
    function _verifyAndAdvanceRatchet(bytes calldata journalBytes, bytes calldata seal)
        internal
        returns (MintCalculatorJournal memory journal, bytes32 commitId)
    {
        commitId = keccak256(journalBytes);
        if (commitProcessed[commitId]) {
            revert CommitAlreadyProcessed(commitId);
        }

        VERIFIER.verify(seal, MINT_CALCULATOR_ID, sha256(journalBytes));
        journal = abi.decode(journalBytes, (MintCalculatorJournal));
        if (!Steel.validateCommitment(journal.steelCommit)) {
            revert InvalidSteelCommitment();
        }
        if (journal.povwAccountingAddress != address(ACCOUNTING)) {
            revert IncorrectSteelContractAddress({
                expected: address(ACCOUNTING), received: journal.povwAccountingAddress
            });
        }
        if (journal.zkcAddress != address(TOKEN)) {
            revert IncorrectSteelContractAddress({expected: address(TOKEN), received: journal.zkcAddress});
        }
        if (journal.zkcRewardsAddress != address(TOKEN_REWARDS)) {
            revert IncorrectSteelContractAddress({
                expected: address(TOKEN_REWARDS), received: journal.zkcRewardsAddress
            });
        }

        // Ensure the initial commit for each update is correct and update the final commit.
        for (uint256 i = 0; i < journal.updates.length; i++) {
            MintCalculatorUpdate memory update = journal.updates[i];

            bytes32 expectedCommit = workLogCommits[update.workLogId];
            if (expectedCommit == bytes32(0)) {
                expectedCommit = EMPTY_LOG_ROOT;
            }

            if (update.initialCommit != expectedCommit) {
                revert IncorrectInitialUpdateCommit({expected: expectedCommit, received: update.initialCommit});
            }
            workLogCommits[update.workLogId] = update.updatedCommit;
        }

        commitProcessed[commitId] = true;
    }

    /// @inheritdoc IPovwMint
    /// @notice Emergency mint at 1x (no pool split). Owner only.
    function mint(bytes calldata journalBytes, bytes calldata seal) external onlyOwner {
        (MintCalculatorJournal memory journal,) = _verifyAndAdvanceRatchet(journalBytes, seal);

        // Issue all of the mint calls at 1x (no pool split).
        for (uint256 i = 0; i < journal.mints.length; i++) {
            MintCalculatorMint memory mintData = journal.mints[i];
            TOKEN.mintPoVWRewardsForRecipient(mintData.recipient, mintData.value);
        }
    }

    /// @inheritdoc IPovwMint
    function commitEpoch(
        bytes calldata mintJournalBytes,
        bytes calldata mintSeal,
        bytes calldata marketJournalBytes,
        bytes calldata marketSeal
    ) external {
        // 1. Verify mint calculator proof + advance ratchet
        (MintCalculatorJournal memory journal, bytes32 commitId) = _verifyAndAdvanceRatchet(mintJournalBytes, mintSeal);

        // 2. Verify market contribution proof
        VERIFIER.verify(marketSeal, MARKET_CONTRIBUTION_CALCULATOR_ID, sha256(marketJournalBytes));
        MarketContributionJournal memory marketJournal = abi.decode(marketJournalBytes, (MarketContributionJournal));
        if (!Steel.validateCommitment(marketJournal.steelCommit)) {
            revert InvalidSteelCommitment();
        }
        if (marketJournal.marketAddress != MARKET_ADDRESS) {
            revert IncorrectMarketAddress({expected: MARKET_ADDRESS, received: marketJournal.marketAddress});
        }

        // 3. Build market contribution total
        uint256 totalMarketContrib = 0;
        for (uint256 i = 0; i < marketJournal.contributions.length; i++) {
            totalMarketContrib += marketJournal.contributions[i].contribution;
        }

        // 4. Compute totals
        uint256 p1Bps = pool1FractionBps;
        uint256 totalBase = 0;
        for (uint256 i = 0; i < journal.mints.length; i++) {
            totalBase += journal.mints[i].value;
        }
        uint256 marketPool = totalBase * (10000 - p1Bps) / 10000;

        // 5. Mint for each prover
        for (uint256 i = 0; i < journal.mints.length; i++) {
            address recipient = journal.mints[i].recipient;
            uint256 base = journal.mints[i].value;

            // Pool 1: proportional to mining base
            uint256 miningAmount = base * p1Bps / 10000;

            // Pool 2: proportional to market contribution
            uint256 marketAmount = 0;
            if (totalMarketContrib > 0) {
                uint256 contrib = _findContribution(marketJournal.contributions, recipient);
                marketAmount = marketPool * contrib / totalMarketContrib;
            }

            TOKEN.mintPoVWRewardsForRecipient(recipient, miningAmount + marketAmount);
        }

        emit EpochCommitted(commitId, totalBase, totalMarketContrib, p1Bps);
    }

    /// @inheritdoc IPovwMint
    function setPool1FractionBps(uint256 bps) external onlyOwner {
        if (bps > 10000) {
            revert InvalidPool1FractionBps(bps);
        }
        emit Pool1FractionBpsUpdated(pool1FractionBps, bps);
        pool1FractionBps = bps;
    }

    /// @inheritdoc IPovwMint
    function workLogCommit(address workLogId) public view returns (bytes32) {
        bytes32 commit = workLogCommits[workLogId];
        if (commit == bytes32(0)) {
            return EMPTY_LOG_ROOT;
        }
        return commit;
    }

    /// @dev Linear scan to find a prover's contribution. Returns 0 if not found.
    function _findContribution(MarketContribution[] memory contributions, address prover)
        internal
        pure
        returns (uint256)
    {
        for (uint256 i = 0; i < contributions.length; i++) {
            if (contributions[i].prover == prover) {
                return contributions[i].contribution;
            }
        }
        return 0;
    }
}
