// Copyright (c) 2024 RISC Zero, Inc.
//
// All rights reserved.

pragma solidity ^0.8.12;

import {IRiscZeroVerifier} from "risc0/IRiscZeroVerifier.sol";
import "./IEigenPod.sol";
import "./ImageID.sol";

library PodLib {
    /// @notice This struct contains the data needed to verify a partial/full withdrawal zk proof
    struct WithdrawalJournal {
        uint40 validatorIndex;
        uint64 withdrawalAmountGwei;
        uint64 withdrawalTimestamp;
        bytes32 beaconStateRoot;
        bool fullWithdrawal;
        bytes32 validatorPubkeyHash;
    }

    uint64 constant YIELD_FACTOR = 10000; // 0.01%
}

contract EigenPod is IEigenPod {
    struct Validator {
        bytes32 pubkeyHash;
        uint40 validatorIndex;
        address withdrawalCreds;
        uint64 stakeGwei;
        uint256 lastUpdateBlock;
        uint64 yieldGwei;
    }

    uint40 nextValidatorIndex = 0;
    uint64 public constant YIELD_FACTOR = PodLib.YIELD_FACTOR;
    IRiscZeroVerifier public verifier;
    bytes32 public constant imageId = ImageID.EIGEN_WITHDRAWAL_ID;
    address public owner;

    // Sequential list of created Validators
    mapping(uint40 => Validator) validators;
    // validator - timestamp mapping for recording partial withdrawals
    mapping(uint40 => uint256) withdrawalAtTimestamp;

    uint256 constant GWEI_TO_WEI = 1e9;

    constructor(IRiscZeroVerifier _verifier) {
        verifier = _verifier;
        owner = msg.sender;
    }

    /**
     * @dev Deposit function to allow the contract to receive Ether
     */
    function topUp() public payable {}

    /**
     * @dev Drain function to allow the owner to withdraw Ether, so that we do not waste
     * testnet funds.
     */
    function drain() public {
        require(msg.sender == owner, "EigenPod.drain: only owner can drain contract");
        (bool sent,) = owner.call{value: address(this).balance}("");
        require(sent, "Failed to send Ether");
    }

    /**
     * @dev Processes a deposit for a new validator and returns the
     * validator index.
     */
    function newValidator() public payable returns (uint40 validatorIndex) {
        uint64 stakeWei = uint64(msg.value);

        // These checks mimic the checks made in the beacon chain deposit contract
        //
        // We sanity-check them here because this contract sorta acts like the
        // deposit contract and this ensures we only create validators that could
        // exist IRL
        require(stakeWei >= 1 ether, "EigenPod.newValidator: deposit value too low");
        require(stakeWei % 1 gwei == 0, "EigenPod.newValidator: value not multiple of gwei");
        uint256 depositAmount = stakeWei / GWEI_TO_WEI;
        require(depositAmount <= type(uint64).max, "EigenPod.newValidator: deposit value too high");

        // Create unique index for new validator
        validatorIndex = nextValidatorIndex;
        nextValidatorIndex++;

        // Create new validator and record in state
        Validator memory validator = Validator({
            pubkeyHash: keccak256(abi.encodePacked(validatorIndex)),
            validatorIndex: validatorIndex,
            withdrawalCreds: msg.sender,
            stakeGwei: uint64(depositAmount),
            lastUpdateBlock: block.number,
            yieldGwei: 0
        });
        validators[validatorIndex] = validator;

        emit NewValidator(validatorIndex, uint64(depositAmount));

        return (validator.validatorIndex);
    }

    /**
     * @dev Full withdrawal, given a journal and seal.
     *
     * This method will send the full withdrawal amount to the validator's withdrawal
     * destination. The passed-in journal and seal should correspond to a zk proof attestating
     * to the validator's credential for a full withdrawal.
     */
    function fullWithdrawal(bytes calldata journal, bytes calldata seal) public {
        bytes32 journalDigest = sha256(abi.encodePacked(journal));
        verifier.verify(seal, imageId, journalDigest);

        PodLib.WithdrawalJournal memory withdrawalJournal = abi.decode(journal, (PodLib.WithdrawalJournal));

        uint40 validatorIndex = withdrawalJournal.validatorIndex;
        require(
            msg.sender == validators[validatorIndex].withdrawalCreds,
            "EigenPod.fullWithdrawal: invalid withdrawal creds"
        );

        updateYield(validatorIndex);
        Validator memory validator = validators[validatorIndex];

        require(withdrawalAtTimestamp[validatorIndex] == 0, "EigenPod.fullWithdrawal: already withdrawn");
        withdrawalAtTimestamp[validatorIndex] = withdrawalJournal.withdrawalTimestamp;

        // Get the withdrawal amount and destination
        uint256 amountToWithdraw = (validator.stakeGwei + validator.yieldGwei) * GWEI_TO_WEI;
        address destination = validator.withdrawalCreds;

        // Update state - set validator stake to zero and send balance to withdrawal destination
        validators[validatorIndex].stakeGwei = 0;
        validators[validatorIndex].yieldGwei = 0;
        (bool sent,) = destination.call{value: amountToWithdraw}("");
        require(sent, "Failed to send Ether");
        emit ValidatorExited(validatorIndex, amountToWithdraw);
    }

    /**
     * @dev Processes a new deposit for an existing validator.
     */
    function updateStake(uint40 validatorIndex) public payable {
        uint64 stakeWei = uint64(msg.value);

        require(stakeWei % 1 gwei == 0, "EigenPod.newValidator: value not multiple of gwei");
        uint256 depositAmount = stakeWei / GWEI_TO_WEI;
        require(depositAmount <= type(uint64).max, "EigenPod.newValidator: deposit value too high");

        updateYield(validatorIndex);
        Validator memory validator = validators[validatorIndex];
        address destination = validator.withdrawalCreds;
        require(msg.sender == destination, "EigenPod.updateStake: invalid withdrawal creds");

        // Update validator balance in state
        uint64 newBalance = validator.stakeGwei + uint64(depositAmount);
        validators[validatorIndex].stakeGwei = newBalance;
        emit StakeUpdated(validatorIndex, newBalance);
    }

    /**
     * @dev Partial withdrawal, given a journal and seal. This method will send the withdrawal amount
     * to the validator's withdrawal destination. The passed-in journal and seal should correspond to a zk proof
     * attesting to the validator's credential for a partial withdrawal.
     */
    function partialWithdrawal(bytes calldata journal, bytes calldata seal) public {
        bytes32 journalDigest = sha256(abi.encodePacked(journal));
        verifier.verify(seal, imageId, journalDigest);

        PodLib.WithdrawalJournal memory withdrawalJournal = abi.decode(journal, (PodLib.WithdrawalJournal));

        uint40 validatorIndex = withdrawalJournal.validatorIndex;
        require(
            msg.sender == validators[validatorIndex].withdrawalCreds,
            "EigenPod.partialWithdrawal: invalid withdrawal creds"
        );

        updateYield(validatorIndex);
        Validator memory validator = validators[validatorIndex];

        require(withdrawalAtTimestamp[validatorIndex] == 0, "EigenPod.partialWithdrawal: already withdrawn");
        withdrawalAtTimestamp[validatorIndex] = withdrawalJournal.withdrawalTimestamp;

        // Get the withdrawal amount and destination
        uint256 amountToWithdraw = validator.yieldGwei * GWEI_TO_WEI;
        address destination = validator.withdrawalCreds;

        // Update state - set validator yield to zero and send balance to withdrawal destination
        validators[validatorIndex].yieldGwei = 0;
        (bool sent,) = destination.call{value: amountToWithdraw}("");
        require(sent, "Failed to send Ether");
        emit Redeemed(validatorIndex, validator.yieldGwei);
    }

    /**
     * @dev Returns the stake of a validator in Gwei.
     */
    function stakeBalanceGwei(uint40 validatorIndex) public view returns (uint64) {
        return validators[validatorIndex].stakeGwei;
    }

    /**
     * @dev Returns the pubkey hash of a validator.
     */
    function pubkeyHash(uint40 validatorIndex) public view returns (bytes32) {
        return validators[validatorIndex].pubkeyHash;
    }

    /**
     * @dev Returns the estimated yield at the current block of a validator in Gwei.
     */
    function estimateYield(uint40 validatorIndex) public view returns (uint64) {
        uint256 currentBlock = block.number;
        uint256 slots = currentBlock - validators[validatorIndex].lastUpdateBlock;
        uint64 yieldGwei = ((validators[validatorIndex].stakeGwei / YIELD_FACTOR) * uint64(slots));
        return validators[validatorIndex].yieldGwei + yieldGwei;
    }

    function updateYield(uint40 validatorIndex) internal {
        uint256 currentBlock = block.number;
        uint256 slots = currentBlock - validators[validatorIndex].lastUpdateBlock;
        uint64 yieldGwei = ((validators[validatorIndex].stakeGwei / YIELD_FACTOR) * uint64(slots));
        validators[validatorIndex].yieldGwei += yieldGwei;
        validators[validatorIndex].lastUpdateBlock = currentBlock;
    }
}
