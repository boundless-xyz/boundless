// Copyright 2026 Boundless Foundation, Inc.
// SPDX-License-Identifier: BUSL-1.1
pragma solidity ^0.8.26;

import {OwnableUpgradeable} from "@openzeppelin/contracts-upgradeable/access/OwnableUpgradeable.sol";
import {Initializable} from "@openzeppelin/contracts-upgradeable/proxy/utils/Initializable.sol";

import {SafeFork} from "./SafeFork.sol";
import {PovwAccounting} from "../src/povw/PovwAccounting.sol";
import {PovwMint} from "../src/povw/PovwMint.sol";
import {Sunset} from "../src/libraries/Sunset.sol";

contract PovwForkTest is SafeFork {
    PovwAccounting constant accounting = PovwAccounting(0x319bd4050b2170a7aE3Ead3E6d5AB8a5c7cFBDF8);
    PovwMint constant minter = PovwMint(0xBFCE7c2d5e7EdDEab71B3eeED770713c8b755397);
    address constant FOUNDATION = 0x3886eEaf95AA2bDDdf0C924925e290291f70447F;
    address constant MULTISEND = 0x40A2aCCbd92BCA938b02010E17A5b8929b49130D;
    address oldAccounting;
    address oldMint;
    address newAccounting;
    address newMint;

    function setUp() public {
        assertEq(block.chainid, 1, "Ethereum fork required");
        assertEq(block.number, 26276732, "pinned block required");
        oldAccounting = implementation(address(accounting));
        oldMint = implementation(address(minter));
        assertEq(oldAccounting, 0xfb2187436fD8e266E4F18cBCe9aAFbDbB796e67C);
        assertEq(oldMint, 0x569D48052d59dC4aAC86098fE8db43D0E514f59B);
        assertEq(oldAccounting.codehash, 0x4bac90ef98d198c558b236c0004335cbbd71f341623eb723679f424ae23654d9);
        assertEq(oldMint.codehash, 0x45300b4466837e64abce4d91f75fd4576438490a5430ba938a892f2dab816cc8);
        assertEq(accounting.owner(), FOUNDATION);
        assertEq(minter.owner(), FOUNDATION);
        assertGt(MULTISEND.code.length, 0, "real MultiSendCallOnly required");
        newAccounting =
            address(new PovwAccounting(accounting.VERIFIER(), accounting.TOKEN(), accounting.LOG_UPDATER_ID()));
        newMint = address(
            new PovwMint(
                minter.VERIFIER(), accounting, minter.MINT_CALCULATOR_ID(), minter.TOKEN(), minter.TOKEN_REWARDS()
            )
        );
    }

    function batch(address a, address m) internal pure returns (bytes memory) {
        bytes memory first = abi.encodeWithSignature("upgradeToAndCall(address,bytes)", a, bytes(""));
        bytes memory second = abi.encodeWithSignature("upgradeToAndCall(address,bytes)", m, bytes(""));
        return abi.encodeWithSignature(
            "multiSend(bytes)",
            abi.encodePacked(
                uint8(0),
                address(accounting),
                uint256(0),
                uint256(first.length),
                first,
                uint8(0),
                address(minter),
                uint256(0),
                uint256(second.length),
                second
            )
        );
    }

    function dependencyHash() internal view returns (bytes32) {
        return keccak256(
            abi.encode(
                accounting.VERIFIER(),
                accounting.TOKEN(),
                accounting.LOG_UPDATER_ID(),
                minter.VERIFIER(),
                minter.TOKEN(),
                minter.TOKEN_REWARDS(),
                minter.ACCOUNTING(),
                minter.MINT_CALCULATOR_ID()
            )
        );
    }

    function upgrade() internal {
        bytes32 beforeEpoch = keccak256(abi.encode(accounting.pendingEpoch()));
        bytes32 dependencies = dependencyHash();
        vm.record();
        executeSafe(FOUNDATION, MULTISEND, batch(newAccounting, newMint), 1, true);
        assertOnlyImplementationWritten(address(accounting));
        assertOnlyImplementationWritten(address(minter));
        assertEq(implementation(address(accounting)), newAccounting);
        assertEq(implementation(address(minter)), newMint);
        assertEq(keccak256(abi.encode(accounting.pendingEpoch())), beforeEpoch);
        assertEq(dependencyHash(), dependencies);
        assertEq(accounting.owner(), FOUNDATION);
        assertEq(minter.owner(), FOUNDATION);
    }

    function testForkAtomicSafeUpgradeAndCutoff() public {
        upgrade();
        vm.warp(T - 1);
        accounting.finalizeEpoch();
        for (uint256 i; i < 2; ++i) {
            vm.warp(T + i);
            bytes32 state = keccak256(abi.encode(accounting.pendingEpoch()));
            vm.recordLogs();
            vm.expectRevert(Sunset.ProtocolSunset.selector);
            accounting.updateWorkLog(address(1), bytes32(uint256(1)), 1, address(1), "");
            vm.expectRevert(Sunset.ProtocolSunset.selector);
            accounting.finalizeEpoch();
            vm.expectRevert(Sunset.ProtocolSunset.selector);
            minter.mint("", "");
            assertEq(keccak256(abi.encode(accounting.pendingEpoch())), state);
            assertEq(vm.getRecordedLogs().length, 0);
        }
    }

    function testForkFailedSecondUpgradeRollsBackWholeSafeBatch() public {
        vm.expectCall(address(accounting), abi.encodeCall(accounting.upgradeToAndCall, (newAccounting, bytes(""))));
        vm.expectCall(address(minter), abi.encodeCall(minter.upgradeToAndCall, (address(0), bytes(""))));
        executeSafe(FOUNDATION, MULTISEND, batch(newAccounting, address(0)), 1, false);
        assertEq(implementation(address(accounting)), oldAccounting);
        assertEq(implementation(address(minter)), oldMint);
    }

    function testForkUnauthorizedUpgradeAndRollback() public {
        bytes memory unauthorized =
            abi.encodeWithSelector(OwnableUpgradeable.OwnableUnauthorizedAccount.selector, address(this));
        vm.expectRevert(unauthorized);
        accounting.upgradeToAndCall(newAccounting, "");
        vm.expectRevert(unauthorized);
        minter.upgradeToAndCall(newMint, "");
        assertEq(implementation(address(accounting)), oldAccounting);
        assertEq(implementation(address(minter)), oldMint);
        upgrade();
        vm.expectRevert(unauthorized);
        accounting.upgradeToAndCall(oldAccounting, "");
        vm.expectRevert(unauthorized);
        minter.upgradeToAndCall(oldMint, "");
        assertEq(implementation(address(accounting)), newAccounting);
        assertEq(implementation(address(minter)), newMint);
        vm.expectRevert(Initializable.InvalidInitialization.selector);
        accounting.initialize(address(1));
        vm.expectRevert(Initializable.InvalidInitialization.selector);
        minter.initialize(address(1));
        vm.expectRevert(Initializable.InvalidInitialization.selector);
        PovwAccounting(newAccounting).initialize(address(1));
        vm.expectRevert(Initializable.InvalidInitialization.selector);
        PovwMint(newMint).initialize(address(1));
        executeSafe(FOUNDATION, MULTISEND, batch(oldAccounting, oldMint), 1, true);
        assertEq(implementation(address(accounting)), oldAccounting);
        assertEq(implementation(address(minter)), oldMint);
        upgrade();
    }
}
