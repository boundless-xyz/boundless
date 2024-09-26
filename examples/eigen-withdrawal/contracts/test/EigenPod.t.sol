// Copyright (c) 2024 RISC Zero, Inc.
//
// All rights reserved.

pragma solidity ^0.8.12;

import "forge-std/Test.sol";
import {PodLib, EigenPod} from "../src/EigenPod.sol";
import {Receipt as RiscZeroReceipt} from "risc0/IRiscZeroVerifier.sol";
import {RiscZeroMockVerifier} from "risc0/test/RiscZeroMockVerifier.sol";
import "../src/ImageID.sol";

contract EigenPodTest is Test {
    EigenPod pod;
    address private owner;
    address private validator;
    RiscZeroMockVerifier verifier;

    function setUp() public {
        verifier = new RiscZeroMockVerifier(bytes4(0));
        owner = address(1);
        validator = address(2);
        vm.deal(owner, 100 ether);
        vm.deal(validator, 3 ether);
        vm.startPrank(owner);
        pod = new EigenPod(verifier);
        pod.topUp{value: 10 ether}();
        vm.stopPrank();
    }

    /// @notice Test draining the contract
    function testDrain() public {
        vm.startPrank(owner);
        pod.drain();
        assertEq(address(pod).balance, 0, "Balance should be 0");
        vm.stopPrank();
    }

    /// @notice Test validator creation and ensure it stores correct information
    function testNewValidator() public {
        vm.startPrank(validator);
        uint40 index = pod.newValidator{value: 1 ether}();
        vm.stopPrank();
        assertEq(uint256(index), 0, "Validator index should start at 0");
        assertEq(pod.stakeBalanceGwei(index), 1e9, "Balance should be 1e9 gwei");
        assertEq(pod.pubkeyHash(index), keccak256(abi.encodePacked(index)), "Pubkey hash mismatch");
    }

    /// @notice Test full withdrawal
    function testFullWithdrawal() public {
        PodLib.WithdrawalJournal memory withdrawalJournal = PodLib.WithdrawalJournal({
            validatorIndex: 0,
            withdrawalAmountGwei: 1e9,
            withdrawalTimestamp: 0,
            beaconStateRoot: 0x0,
            fullWithdrawal: true,
            validatorPubkeyHash: keccak256(abi.encodePacked(validator))
        });
        bytes memory journal = abi.encode(withdrawalJournal);
        bytes32 journalDigest = sha256(journal);
        // create a mock proof
        RiscZeroReceipt memory receipt = verifier.mockProve(ImageID.EIGEN_WITHDRAWAL_ID, journalDigest);

        vm.startPrank(validator);
        uint40 index = pod.newValidator{value: 1 ether}();
        pod.fullWithdrawal(journal, receipt.seal);
        vm.stopPrank();

        assertEq(pod.stakeBalanceGwei(index), 0, "Balance should be 0 after exit");
        assertTrue(address(this).balance > 0, "Ether should be withdrawn to this address");
    }

    /// @notice Test stake updates
    function testUpdateStake() public {
        vm.startPrank(validator);
        uint40 index = pod.newValidator{value: 1 ether}();
        pod.updateStake{value: 1 ether}(index);
        assertEq(pod.stakeBalanceGwei(index), 1e9 + 1e9, "Balance should increase by 1e9 gwei");
        vm.stopPrank();
    }

    /// @notice Test partial withdrawal
    function testPartialWithdrawal() public {
        PodLib.WithdrawalJournal memory withdrawalJournal = PodLib.WithdrawalJournal({
            validatorIndex: 0,
            withdrawalAmountGwei: 1e9,
            withdrawalTimestamp: 0,
            beaconStateRoot: 0x0,
            fullWithdrawal: true,
            validatorPubkeyHash: keccak256(abi.encodePacked(validator))
        });
        bytes memory journal = abi.encode(withdrawalJournal);
        bytes32 journalDigest = sha256(journal);
        // create a mock proof
        RiscZeroReceipt memory receipt = verifier.mockProve(ImageID.EIGEN_WITHDRAWAL_ID, journalDigest);

        vm.startPrank(validator);
        pod.newValidator{value: 1 ether}();
        vm.roll(block.number + 10);
        pod.partialWithdrawal(journal, receipt.seal);
        uint256 expectedYield = (1 ether / PodLib.YIELD_FACTOR) * 10;
        assertEq(validator.balance, 2 ether + expectedYield, "Validator should yield (1 ether / YIELD_FACTOR) * 10");
        vm.stopPrank();
    }
}
