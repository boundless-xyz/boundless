// Copyright 2026 Boundless Foundation, Inc.
//
// Use of this source code is governed by the Business Source License
// as found in the LICENSE-BSL file.

pragma solidity ^0.8.26;

import {SafeFork} from "./SafeFork.sol";
import {ISunsetSafe} from "../scripts/sunset/SunsetTransaction.sol";
import {SunsetTransaction} from "../scripts/sunset/SunsetTransaction.sol";
import {PovwAccounting} from "../src/povw/PovwAccounting.sol";
import {PovwMint} from "../src/povw/PovwMint.sol";
import {Sunset} from "../src/libraries/Sunset.sol";

interface IBatchZKC {
    function getTotalPoVWEmissionsAtEpochStart(uint256) external view returns (uint256);
    function getTotalStakingEmissionsAtEpochStart(uint256) external view returns (uint256);
    function mintPoVWRewardsForRecipient(address, uint256) external;
    function balanceOf(address) external view returns (uint256);
}

contract EthereumBatchForkTest is SafeFork {
    address constant SAFE = 0x3886eEaf95AA2bDDdf0C924925e290291f70447F;
    address constant MULTISEND = 0x40A2aCCbd92BCA938b02010E17A5b8929b49130D;
    address constant ACCOUNTING = 0x319bd4050b2170a7aE3Ead3E6d5AB8a5c7cFBDF8;
    address constant MINT = 0xBFCE7c2d5e7EdDEab71B3eeED770713c8b755397;
    address constant ZKC = 0x000006c2A22ff4A44ff1f5d0F2ed65F781F55555;
    bytes bundle;
    address[3] proxies = [ACCOUNTING, MINT, ZKC];
    address[3] candidates;
    address[3] baselines;

    function setUp() public {
        assertEq(block.chainid, 1);
        string memory json = vm.readFile(vm.envString("SUNSET_BUNDLE"));
        assertEq(block.number, vm.parseJsonUint(json, ".blockNumber"));
        assertEq(vm.parseJsonAddress(json, ".safe"), SAFE);
        assertEq(vm.parseJsonAddress(json, ".to"), MULTISEND);
        assertEq(vm.parseJsonUint(json, ".operation"), 1);
        assertEq(vm.parseJsonUint(json, ".value"), 0);
        assertEq(vm.parseJsonUint(json, ".chainId"), 1);
        assertEq(vm.parseJsonUint(json, ".safeTxGas"), 0);
        assertEq(vm.parseJsonUint(json, ".baseGas"), 0);
        assertEq(vm.parseJsonUint(json, ".gasPrice"), 0);
        assertEq(vm.parseJsonAddress(json, ".gasToken"), address(0));
        assertEq(vm.parseJsonAddress(json, ".refundReceiver"), address(0));
        bundle = vm.parseJsonBytes(json, ".data");
        uint256 nonce = ISunsetSafe(SAFE).nonce();
        assertEq(vm.parseJsonUint(json, ".nonce"), nonce);
        assertEq(
            vm.parseJsonBytes32(json, ".safeTxHash"),
            ISunsetSafe(SAFE).getTransactionHash(MULTISEND, 0, bundle, 1, 0, 0, 0, address(0), address(0), nonce)
        );
        for (uint256 i; i < 3; ++i) {
            string memory key = string.concat(".steps[", vm.toString(i), "]");
            assertEq(vm.parseJsonAddress(json, string.concat(key, ".proxy")), proxies[i]);
            candidates[i] = vm.parseJsonAddress(json, string.concat(key, ".implementation"));
            assertGt(candidates[i].code.length, 0);
            baselines[i] = implementation(proxies[i]);
        }
        assertEq(baselines[0], 0xfb2187436fD8e266E4F18cBCe9aAFbDbB796e67C);
        assertEq(baselines[1], 0x569D48052d59dC4aAC86098fE8db43D0E514f59B);
        assertEq(baselines[2], 0xe90A3bc5992d30b9909eeb6A8015A7bC402D7F98);
        assertEq(baselines[0].codehash, 0x4bac90ef98d198c558b236c0004335cbbd71f341623eb723679f424ae23654d9);
        assertEq(baselines[1].codehash, 0x45300b4466837e64abce4d91f75fd4576438490a5430ba938a892f2dab816cc8);
        assertEq(baselines[2].codehash, 0x809103882fcb7d3a50c452a7db932916ec00c50abacbb3452508085a6aa79529);
    }

    function testForkExactExportedBatchAndCutoff() public {
        uint256 staking = IBatchZKC(ZKC).getTotalStakingEmissionsAtEpochStart(364);
        uint256 balance = IBatchZKC(ZKC).balanceOf(SAFE);
        vm.record();
        executeSafe(SAFE, MULTISEND, bundle, 1, true);
        for (uint256 i; i < 3; ++i) {
            assertEq(implementation(proxies[i]), candidates[i]);
            assertOnlyImplementationWritten(proxies[i]);
        }
        assertEq(IBatchZKC(ZKC).balanceOf(SAFE), balance);
        assertEq(IBatchZKC(ZKC).getTotalStakingEmissionsAtEpochStart(364), staking);
        assertEq(
            IBatchZKC(ZKC).getTotalPoVWEmissionsAtEpochStart(364), IBatchZKC(ZKC).getTotalPoVWEmissionsAtEpochStart(210)
        );
        vm.warp(T);
        vm.expectRevert(Sunset.ProtocolSunset.selector);
        PovwAccounting(ACCOUNTING).finalizeEpoch();
        vm.expectRevert(Sunset.ProtocolSunset.selector);
        PovwMint(MINT).mint("", "");
        vm.prank(MINT);
        vm.expectRevert(bytes4(keccak256("PoVWSunset()")));
        IBatchZKC(ZKC).mintPoVWRewardsForRecipient(SAFE, 0);
    }

    function testForkFailedThirdUpgradeRollsBackAllThree() public {
        bytes memory third = SunsetTransaction.upgrade(address(0));
        bytes memory firstTwo = abi.decode(slice(bundle, 4), (bytes));
        // The last packed call is always 85 bytes of framing plus 100 bytes of upgrade calldata.
        bytes memory packed = sliceLength(firstTwo, 0, firstTwo.length - 185);
        bytes memory failed = abi.encodeWithSignature(
            "multiSend(bytes)", abi.encodePacked(packed, uint8(0), ZKC, uint256(0), uint256(third.length), third)
        );
        uint256 nonce = ISunsetSafe(SAFE).nonce();
        vm.expectCall(ZKC, third);
        vm.expectRevert(bytes("GS013"));
        this.executeInvalidBatch(failed);
        assertEq(ISunsetSafe(SAFE).nonce(), nonce);
        for (uint256 i; i < 3; ++i) {
            assertEq(implementation(proxies[i]), baselines[i]);
        }
    }

    function executeInvalidBatch(bytes calldata failed) external {
        // Keep the production safeTxGas=0 path, which reverts the complete Safe execution.
        executeSafe(SAFE, MULTISEND, failed, 1, true);
    }

    function slice(bytes memory data, uint256 offset) internal pure returns (bytes memory) {
        return sliceLength(data, offset, data.length - offset);
    }

    function sliceLength(bytes memory data, uint256 offset, uint256 length)
        internal
        pure
        returns (bytes memory result)
    {
        result = new bytes(length);
        for (uint256 i; i < length; ++i) {
            result[i] = data[offset + i];
        }
    }
}
