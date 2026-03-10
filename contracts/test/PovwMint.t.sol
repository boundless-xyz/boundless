// Copyright 2026 Boundless Foundation, Inc.
//
// Use of this source code is governed by the Business Source License
// as found in the LICENSE-BSL file.
// SPDX-License-Identifier: BUSL-1.1

pragma solidity ^0.8.26;

import {Test} from "forge-std/Test.sol";
import {ERC1967Proxy} from "@openzeppelin/contracts/proxy/ERC1967/ERC1967Proxy.sol";
import {RiscZeroMockVerifier} from "risc0/test/RiscZeroMockVerifier.sol";
import {Steel, Encoding, ChainSpec} from "steel/Steel.sol";
import {PovwAccounting, EMPTY_LOG_ROOT} from "../src/povw/PovwAccounting.sol";
import {
    PovwMint,
    IPovwMint,
    MintCalculatorUpdate,
    MintCalculatorMint,
    MintCalculatorJournal,
    MarketContribution,
    MarketContributionJournal
} from "../src/povw/PovwMint.sol";
import {MockZKC, MockZKCRewards} from "./MockZKC.sol";
import {IZKC} from "zkc/interfaces/IZKC.sol";
import {IRewards as IZKCRewards} from "zkc/interfaces/IRewards.sol";

contract PovwMintTest is Test {
    RiscZeroMockVerifier internal verifier;
    MockZKC internal zkc;
    MockZKCRewards internal zkcRewards;
    PovwAccounting internal accounting;
    PovwMint internal povwMint;

    bytes32 constant MINT_CALCULATOR_ID = bytes32(uint256(0x1111));
    bytes32 constant MARKET_CONTRIBUTION_CALCULATOR_ID = bytes32(uint256(0x2222));
    bytes32 constant LOG_UPDATER_ID = bytes32(uint256(0x3333));
    address constant MARKET_ADDRESS = address(0xBEEF);
    address constant OWNER = address(0xA11CE);

    address constant PROVER_A = address(0x1001);
    address constant PROVER_B = address(0x1002);

    function setUp() public {
        vm.chainId(ChainSpec.STEEL_TEST_PRAGUE_CHAIN_ID);
        // Roll forward so we have valid block hashes available
        vm.roll(100);

        verifier = new RiscZeroMockVerifier(bytes4(0));
        zkc = new MockZKC();
        zkcRewards = new MockZKCRewards();

        // Deploy PovwAccounting (needed as immutable in PovwMint)
        address accountingImpl = address(new PovwAccounting(verifier, IZKC(address(zkc)), LOG_UPDATER_ID));
        accounting = PovwAccounting(
            address(new ERC1967Proxy(accountingImpl, abi.encodeCall(PovwAccounting.initialize, (OWNER))))
        );

        // Deploy PovwMint
        address mintImpl = address(
            new PovwMint(
                verifier,
                accounting,
                MINT_CALCULATOR_ID,
                MARKET_CONTRIBUTION_CALCULATOR_ID,
                MARKET_ADDRESS,
                IZKC(address(zkc)),
                IZKCRewards(address(zkcRewards))
            )
        );
        povwMint = PovwMint(address(new ERC1967Proxy(mintImpl, abi.encodeCall(PovwMint.initialize, (OWNER)))));

        // Initialize V2 with 30% mining pool
        vm.prank(OWNER);
        povwMint.initializeV2(3000);
    }

    // =========================================================================
    // Helpers
    // =========================================================================

    function _steelCommitment() internal view returns (Steel.Commitment memory) {
        uint256 blockNum = block.number - 1;
        return Steel.Commitment({
            id: Encoding.encodeVersionedID(uint240(blockNum), 0),
            digest: blockhash(blockNum),
            configID: ChainSpec.configID()
        });
    }

    function _makeMintJournal(MintCalculatorMint[] memory mints)
        internal
        view
        returns (bytes memory journalBytes, bytes memory seal)
    {
        MintCalculatorUpdate[] memory updates = new MintCalculatorUpdate[](0);
        MintCalculatorJournal memory journal = MintCalculatorJournal({
            mints: mints,
            updates: updates,
            povwAccountingAddress: address(accounting),
            zkcRewardsAddress: address(zkcRewards),
            zkcAddress: address(zkc),
            steelCommit: _steelCommitment()
        });
        journalBytes = abi.encode(journal);

        seal = verifier.mockProve(MINT_CALCULATOR_ID, sha256(journalBytes)).seal;
    }

    function _makeMarketJournal(MarketContribution[] memory contributions)
        internal
        view
        returns (bytes memory journalBytes, bytes memory seal)
    {
        MarketContributionJournal memory journal = MarketContributionJournal({
            contributions: contributions,
            marketAddress: MARKET_ADDRESS,
            epochStart: 0,
            epochEnd: 100,
            steelCommit: _steelCommitment()
        });
        journalBytes = abi.encode(journal);

        seal = verifier.mockProve(MARKET_CONTRIBUTION_CALCULATOR_ID, sha256(journalBytes)).seal;
    }

    function _singleMint(address recipient, uint256 value) internal pure returns (MintCalculatorMint[] memory mints) {
        mints = new MintCalculatorMint[](1);
        mints[0] = MintCalculatorMint({recipient: recipient, value: value});
    }

    function _twoMints(address recipientA, uint256 valueA, address recipientB, uint256 valueB)
        internal
        pure
        returns (MintCalculatorMint[] memory mints)
    {
        mints = new MintCalculatorMint[](2);
        mints[0] = MintCalculatorMint({recipient: recipientA, value: valueA});
        mints[1] = MintCalculatorMint({recipient: recipientB, value: valueB});
    }

    function _singleContribution(address prover, uint256 contribution)
        internal
        pure
        returns (MarketContribution[] memory contributions)
    {
        contributions = new MarketContribution[](1);
        contributions[0] = MarketContribution({prover: prover, contribution: contribution});
    }

    function _twoContributions(address proverA, uint256 contribA, address proverB, uint256 contribB)
        internal
        pure
        returns (MarketContribution[] memory contributions)
    {
        contributions = new MarketContribution[](2);
        contributions[0] = MarketContribution({prover: proverA, contribution: contribA});
        contributions[1] = MarketContribution({prover: proverB, contribution: contribB});
    }

    // =========================================================================
    // commitEpoch: happy path
    // =========================================================================

    function testCommitEpochHappyPath() public {
        // pool1FractionBps = 3000 (30% mining, 70% market)
        // Two provers: A gets 1000 base, B gets 1000 base
        // Market contrib: A=100, B=0
        // Pool 1 (mining): A = 1000 * 3000/10000 = 300, B = 300
        // Pool 2 total = 2000 * 7000/10000 = 1400
        // A market share = 1400 * 100/100 = 1400
        // B market share = 0
        // Total: A = 300 + 1400 = 1700, B = 300

        (bytes memory mintJournal, bytes memory mintSeal) = _makeMintJournal(_twoMints(PROVER_A, 1000, PROVER_B, 1000));
        (bytes memory marketJournal, bytes memory marketSeal) = _makeMarketJournal(_singleContribution(PROVER_A, 100));

        povwMint.commitEpoch(mintJournal, mintSeal, marketJournal, marketSeal);

        assertEq(zkc.balanceOf(PROVER_A), 1700);
        assertEq(zkc.balanceOf(PROVER_B), 300);
    }

    function testCommitEpochBothProversHaveContribution() public {
        // A=1000 base, B=1000 base
        // Market contrib: A=300, B=100 (total=400)
        // Pool 1: A=300, B=300
        // Pool 2 total = 2000 * 7000/10000 = 1400
        // A market = 1400 * 300/400 = 1050
        // B market = 1400 * 100/400 = 350
        // Total: A=1350, B=650

        (bytes memory mintJournal, bytes memory mintSeal) = _makeMintJournal(_twoMints(PROVER_A, 1000, PROVER_B, 1000));
        (bytes memory marketJournal, bytes memory marketSeal) =
            _makeMarketJournal(_twoContributions(PROVER_A, 300, PROVER_B, 100));

        povwMint.commitEpoch(mintJournal, mintSeal, marketJournal, marketSeal);

        assertEq(zkc.balanceOf(PROVER_A), 1350);
        assertEq(zkc.balanceOf(PROVER_B), 650);
    }

    // =========================================================================
    // commitEpoch: replay reverts
    // =========================================================================

    function testCommitEpochReplayReverts() public {
        (bytes memory mintJournal, bytes memory mintSeal) = _makeMintJournal(_singleMint(PROVER_A, 1000));
        (bytes memory marketJournal, bytes memory marketSeal) = _makeMarketJournal(_singleContribution(PROVER_A, 100));

        povwMint.commitEpoch(mintJournal, mintSeal, marketJournal, marketSeal);

        bytes32 commitId = keccak256(mintJournal);
        vm.expectRevert(abi.encodeWithSelector(IPovwMint.CommitAlreadyProcessed.selector, commitId));
        povwMint.commitEpoch(mintJournal, mintSeal, marketJournal, marketSeal);
    }

    // =========================================================================
    // mint: owner only
    // =========================================================================

    function testMintOwnerOnly() public {
        (bytes memory journalBytes, bytes memory seal) = _makeMintJournal(_singleMint(PROVER_A, 500));

        vm.prank(address(0xBAD));
        vm.expectRevert();
        povwMint.mint(journalBytes, seal);
    }

    function testMintByOwner() public {
        (bytes memory journalBytes, bytes memory seal) = _makeMintJournal(_singleMint(PROVER_A, 500));

        vm.prank(OWNER);
        povwMint.mint(journalBytes, seal);

        // Mints at 1x (no pool split)
        assertEq(zkc.balanceOf(PROVER_A), 500);
    }

    // =========================================================================
    // mint / commitEpoch mutual exclusion
    // =========================================================================

    function testMintThenCommitEpochReverts() public {
        (bytes memory mintJournal, bytes memory mintSeal) = _makeMintJournal(_singleMint(PROVER_A, 1000));
        (bytes memory marketJournal, bytes memory marketSeal) = _makeMarketJournal(_singleContribution(PROVER_A, 100));

        // Use mint first
        vm.prank(OWNER);
        povwMint.mint(mintJournal, mintSeal);

        // commitEpoch with same mint journal should revert
        bytes32 commitId = keccak256(mintJournal);
        vm.expectRevert(abi.encodeWithSelector(IPovwMint.CommitAlreadyProcessed.selector, commitId));
        povwMint.commitEpoch(mintJournal, mintSeal, marketJournal, marketSeal);
    }

    function testCommitEpochThenMintReverts() public {
        (bytes memory mintJournal, bytes memory mintSeal) = _makeMintJournal(_singleMint(PROVER_A, 1000));
        (bytes memory marketJournal, bytes memory marketSeal) = _makeMarketJournal(_singleContribution(PROVER_A, 100));

        povwMint.commitEpoch(mintJournal, mintSeal, marketJournal, marketSeal);

        bytes32 commitId = keccak256(mintJournal);
        vm.expectRevert(abi.encodeWithSelector(IPovwMint.CommitAlreadyProcessed.selector, commitId));
        vm.prank(OWNER);
        povwMint.mint(mintJournal, mintSeal);
    }

    // =========================================================================
    // pool1FractionBps edge cases
    // =========================================================================

    function testPool1FractionBps10000_AllMining() public {
        // 100% mining: all rewards go to mining, Pool 2 = 0
        vm.prank(OWNER);
        povwMint.setPool1FractionBps(10000);

        (bytes memory mintJournal, bytes memory mintSeal) = _makeMintJournal(_singleMint(PROVER_A, 1000));
        (bytes memory marketJournal, bytes memory marketSeal) = _makeMarketJournal(_singleContribution(PROVER_A, 100));

        povwMint.commitEpoch(mintJournal, mintSeal, marketJournal, marketSeal);

        assertEq(zkc.balanceOf(PROVER_A), 1000);
    }

    function testPool1FractionBps0_AllMarket() public {
        // 0% mining: all rewards go to market pool
        vm.prank(OWNER);
        povwMint.setPool1FractionBps(0);

        (bytes memory mintJournal, bytes memory mintSeal) = _makeMintJournal(_twoMints(PROVER_A, 1000, PROVER_B, 1000));
        (bytes memory marketJournal, bytes memory marketSeal) = _makeMarketJournal(_singleContribution(PROVER_A, 100));

        povwMint.commitEpoch(mintJournal, mintSeal, marketJournal, marketSeal);

        // Pool 1 (mining): 0 for both
        // Pool 2 total = 2000 * 10000/10000 = 2000
        // A gets all of pool 2 since they have all market contribution
        // B gets 0
        assertEq(zkc.balanceOf(PROVER_A), 2000);
        assertEq(zkc.balanceOf(PROVER_B), 0);
    }

    function testPool1FractionBps3000_Default() public view {
        assertEq(povwMint.pool1FractionBps(), 3000);
    }

    function testSetPool1FractionBpsAbove10000Reverts() public {
        vm.prank(OWNER);
        vm.expectRevert(abi.encodeWithSelector(IPovwMint.InvalidPool1FractionBps.selector, 10001));
        povwMint.setPool1FractionBps(10001);
    }

    // =========================================================================
    // No market contribution: Pool 2 unminted
    // =========================================================================

    function testZeroTotalMarketContribution() public {
        // With pool1FractionBps = 3000, if no market contribution:
        // Pool 1: 1000 * 3000/10000 = 300
        // Pool 2: unminted (totalMarketContrib == 0)
        // Total: 300 (deflationary)

        MarketContribution[] memory empty = new MarketContribution[](0);
        (bytes memory mintJournal, bytes memory mintSeal) = _makeMintJournal(_singleMint(PROVER_A, 1000));
        (bytes memory marketJournal, bytes memory marketSeal) = _makeMarketJournal(empty);

        povwMint.commitEpoch(mintJournal, mintSeal, marketJournal, marketSeal);

        assertEq(zkc.balanceOf(PROVER_A), 300);
    }

    function testProverWithNoMarketContribGetsPool1Only() public {
        // PROVER_B has base rewards but zero market contribution
        // A=500 base, B=500 base
        // Market contrib: A=100, B not in list => 0
        // Pool 1: A=150, B=150
        // Pool 2 total = 1000 * 7000/10000 = 700
        // A market = 700 * 100/100 = 700
        // B market = 0
        // Total: A=850, B=150

        (bytes memory mintJournal, bytes memory mintSeal) = _makeMintJournal(_twoMints(PROVER_A, 500, PROVER_B, 500));
        (bytes memory marketJournal, bytes memory marketSeal) = _makeMarketJournal(_singleContribution(PROVER_A, 100));

        povwMint.commitEpoch(mintJournal, mintSeal, marketJournal, marketSeal);

        assertEq(zkc.balanceOf(PROVER_A), 850);
        assertEq(zkc.balanceOf(PROVER_B), 150);
    }

    // =========================================================================
    // Invalid market proof / wrong market address
    // =========================================================================

    function testWrongMarketAddressReverts() public {
        (bytes memory mintJournal, bytes memory mintSeal) = _makeMintJournal(_singleMint(PROVER_A, 1000));

        // Create market journal with wrong address
        MarketContribution[] memory contributions = _singleContribution(PROVER_A, 100);
        MarketContributionJournal memory wrongJournal = MarketContributionJournal({
            contributions: contributions,
            marketAddress: address(0xDEAD),
            epochStart: 0,
            epochEnd: 100,
            steelCommit: _steelCommitment()
        });
        bytes memory marketJournalBytes = abi.encode(wrongJournal);
        bytes memory marketSeal = verifier.mockProve(MARKET_CONTRIBUTION_CALCULATOR_ID, sha256(marketJournalBytes)).seal;

        vm.expectRevert(
            abi.encodeWithSelector(IPovwMint.IncorrectMarketAddress.selector, MARKET_ADDRESS, address(0xDEAD))
        );
        povwMint.commitEpoch(mintJournal, mintSeal, marketJournalBytes, marketSeal);
    }

    // =========================================================================
    // setPool1FractionBps access control
    // =========================================================================

    function testSetPool1FractionBpsNotOwnerReverts() public {
        vm.prank(address(0xBAD));
        vm.expectRevert();
        povwMint.setPool1FractionBps(5000);
    }

    function testSetPool1FractionBpsEmitsEvent() public {
        vm.prank(OWNER);
        vm.expectEmit(false, false, false, true);
        emit IPovwMint.Pool1FractionBpsUpdated(3000, 5000);
        povwMint.setPool1FractionBps(5000);
        assertEq(povwMint.pool1FractionBps(), 5000);
    }

    // =========================================================================
    // EpochCommitted event
    // =========================================================================

    function testCommitEpochEmitsEvent() public {
        (bytes memory mintJournal, bytes memory mintSeal) = _makeMintJournal(_twoMints(PROVER_A, 1000, PROVER_B, 500));
        (bytes memory marketJournal, bytes memory marketSeal) = _makeMarketJournal(_singleContribution(PROVER_A, 200));

        bytes32 commitId = keccak256(mintJournal);
        vm.expectEmit(true, false, false, true);
        emit IPovwMint.EpochCommitted(commitId, 1500, 200, 3000);
        povwMint.commitEpoch(mintJournal, mintSeal, marketJournal, marketSeal);
    }

    // =========================================================================
    // VERSION
    // =========================================================================

    function testVersion() public view {
        assertEq(povwMint.VERSION(), 2);
    }

    // =========================================================================
    // initializeV2 cannot be called twice
    // =========================================================================

    function testInitializeV2CannotBeCalledTwice() public {
        vm.prank(OWNER);
        vm.expectRevert();
        povwMint.initializeV2(5000);
    }
}
