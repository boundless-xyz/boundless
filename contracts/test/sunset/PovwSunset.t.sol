// Copyright 2026 Boundless Foundation, Inc.
//
// Use of this source code is governed by the Business Source License
// as found in the LICENSE-BSL file.

pragma solidity ^0.8.26;

import "forge-std/Test.sol";
import {PovwAccounting, EMPTY_LOG_ROOT} from "../../src/povw/PovwAccounting.sol";
import {PovwMint} from "../../src/povw/PovwMint.sol";
import {PovwAccountingOriginal} from "../reference/PovwAccountingOriginal.sol";
import {PovwMintOriginal} from "../reference/PovwMintOriginal.sol";
import {MintCalculatorJournal, MintCalculatorMint, MintCalculatorUpdate} from "../../src/povw/IPovwMint.sol";
import {PendingEpoch} from "../../src/povw/IPovwAccounting.sol";
import {Sunset} from "../../src/libraries/Sunset.sol";
import {Steel, ChainSpec} from "steel/Steel.sol";
import {IRiscZeroVerifier} from "risc0/IRiscZeroVerifier.sol";
import {RiscZeroMockVerifier} from "risc0/test/RiscZeroMockVerifier.sol";
import {MockZKC} from "../MockZKC.sol";
import {IZKC} from "zkc/interfaces/IZKC.sol";
import {IRewards} from "zkc/interfaces/IRewards.sol";
import {ERC1967Proxy} from "@openzeppelin/contracts/proxy/ERC1967/ERC1967Proxy.sol";

contract PovwSunsetTest is Test {
    uint256 constant T = 1793836800;
    address owner = makeAddr("owner");
    address user = makeAddr("user");
    PovwAccounting accounting;
    PovwMint minter;
    IRiscZeroVerifier verifier;
    MockZKC token;
    bytes32 constant IMAGE_ID = bytes32(uint256(1));
    address constant REWARDS = address(0x1234);

    function setUp() public {
        vm.warp(T - 3 days);
        vm.chainId(1);
        vm.roll(100);
        vm.setBlockhash(99, bytes32(uint256(42)));
        verifier = new RiscZeroMockVerifier(bytes4(0));
        // Proof generation is outside this contract-transition suite.
        vm.mockCall(address(verifier), abi.encodeWithSelector(IRiscZeroVerifier.verify.selector), "");
        token = new MockZKC();
        PovwAccountingOriginal a = new PovwAccountingOriginal(verifier, IZKC(address(token)), IMAGE_ID);
        accounting = PovwAccounting(
            address(new ERC1967Proxy(address(a), abi.encodeCall(PovwAccountingOriginal.initialize, (owner))))
        );
        PovwMintOriginal m =
            new PovwMintOriginal(verifier, accounting, IMAGE_ID, IZKC(address(token)), IRewards(REWARDS));
        minter = PovwMint(address(new ERC1967Proxy(address(m), abi.encodeCall(PovwMintOriginal.initialize, (owner)))));
    }

    function upgradeBoth() internal {
        PovwAccounting a = new PovwAccounting(verifier, IZKC(address(token)), IMAGE_ID);
        PovwMint m = new PovwMint(verifier, accounting, IMAGE_ID, IZKC(address(token)), IRewards(REWARDS));
        vm.startPrank(owner);
        accounting.upgradeToAndCall(address(a), "");
        minter.upgradeToAndCall(address(m), "");
        vm.stopPrank();
    }

    function journal(bool empty) internal view returns (bytes memory) {
        MintCalculatorJournal memory j;
        j.povwAccountingAddress = address(accounting);
        j.zkcAddress = address(token);
        j.zkcRewardsAddress = REWARDS;
        j.steelCommit = Steel.Commitment(99, bytes32(uint256(42)), ChainSpec.configID());
        j.updates = new MintCalculatorUpdate[](empty ? 0 : 1);
        j.mints = new MintCalculatorMint[](empty ? 0 : 1);
        if (!empty) {
            j.updates[0] = MintCalculatorUpdate(user, EMPTY_LOG_ROOT, bytes32(uint256(3)));
            j.mints[0] = MintCalculatorMint(user, 100);
        }
        return abi.encode(j);
    }

    function testUpgradeRetainsOwnerDependenciesAndHistoricalState() public {
        accounting.updateWorkLog(user, bytes32(uint256(2)), 7, user, "");
        minter.mint(journal(false), "");
        PendingEpoch memory beforeEpoch = accounting.pendingEpoch();
        upgradeBoth();
        assertEq(accounting.owner(), owner);
        assertEq(minter.owner(), owner);
        assertEq(accounting.workLogCommit(user), bytes32(uint256(2)));
        assertEq(minter.workLogCommit(user), bytes32(uint256(3)));
        assertEq(accounting.pendingEpoch().totalWork, beforeEpoch.totalWork);
        assertEq(accounting.pendingEpoch().number, beforeEpoch.number);
        assertEq(address(accounting.VERIFIER()), address(verifier));
        assertEq(address(minter.TOKEN()), address(token));
    }

    function testWritesAndClaimsWorkImmediatelyBeforeCutoff() public {
        upgradeBoth();
        vm.warp(T - 1);
        accounting.finalizeEpoch();
        accounting.updateWorkLog(user, bytes32(uint256(2)), 7, user, "");
        minter.mint(journal(false), "");
        assertEq(token.balanceOf(user), 100);
        assertEq(accounting.pendingEpoch().totalWork, 7);
    }

    function testAllWritesRevertAtAndAfterCutoff() public {
        bytes memory oldReceipt = journal(false);
        upgradeBoth();
        for (uint256 i; i < 2; ++i) {
            vm.warp(T + i);
            vm.recordLogs();
            vm.expectRevert(Sunset.ProtocolSunset.selector);
            accounting.updateWorkLog(user, bytes32(uint256(2)), 7, user, "");
            vm.expectRevert(Sunset.ProtocolSunset.selector);
            accounting.finalizeEpoch();
            vm.expectRevert(Sunset.ProtocolSunset.selector);
            minter.mint(oldReceipt, "");
            vm.expectRevert(Sunset.ProtocolSunset.selector);
            minter.mint(journal(true), "");
            // Malformed/invalid proofs are rejected at the cutoff before verification/decoding.
            vm.expectRevert(Sunset.ProtocolSunset.selector);
            minter.mint(hex"01", hex"02");
            assertEq(vm.getRecordedLogs().length, 0);
            assertEq(accounting.workLogCommit(user), EMPTY_LOG_ROOT);
            assertEq(minter.workLogCommit(user), EMPTY_LOG_ROOT);
            assertEq(token.balanceOf(user), 0);
        }
    }

    function testEmptyAndZeroMintJournalsRejectedAfterCutoff() public {
        bytes memory encoded = journal(false);
        MintCalculatorJournal memory j = abi.decode(encoded, (MintCalculatorJournal));
        j.mints[0].value = 0;
        upgradeBoth();
        vm.warp(T);
        vm.expectRevert(Sunset.ProtocolSunset.selector);
        minter.mint(abi.encode(j), "");
        vm.expectRevert(Sunset.ProtocolSunset.selector);
        minter.mint(journal(true), "");
    }

    function testOwnerCanUpgradeAfterCutoffButOthersCannot() public {
        PovwAccounting a = new PovwAccounting(verifier, IZKC(address(token)), IMAGE_ID);
        PovwMint m = new PovwMint(verifier, accounting, IMAGE_ID, IZKC(address(token)), IRewards(REWARDS));
        upgradeBoth();
        vm.warp(T);
        vm.expectRevert();
        accounting.upgradeToAndCall(address(a), "");
        vm.expectRevert();
        minter.upgradeToAndCall(address(m), "");
        vm.startPrank(owner);
        accounting.upgradeToAndCall(address(a), "");
        minter.upgradeToAndCall(address(m), "");
        vm.stopPrank();
        assertEq(accounting.owner(), owner);
        assertEq(minter.owner(), owner);
    }
}
