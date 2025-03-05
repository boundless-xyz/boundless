// Copyright (c) 2025 RISC Zero, Inc.
//
// All rights reserved.
pragma solidity ^0.8.13;

import {IBoundlessMarket} from "../../src/IBoundlessMarket.sol";
import {HitPoints} from "../../src/HitPoints.sol";
import "forge-std/Test.sol";
import "forge-std/console.sol";
import {SafeCast} from "@openzeppelin/contracts/utils/math/SafeCast.sol";
import {BoundlessMarket} from "../../src/BoundlessMarket.sol";
import {RequestId, RequestIdLibrary} from "../../src/types/RequestId.sol";
import {AssessorJournal} from "../../src/types/AssessorJournal.sol";
import {BoundlessMarketLib} from "../../src/libraries/BoundlessMarketLib.sol";
import {MerkleProofish} from "../../src/libraries/MerkleProofish.sol";
import {RequestId} from "../../src/types/RequestId.sol";
import {ProofRequest} from "../../src/types/ProofRequest.sol";
import {Account} from "../../src/types/Account.sol";
import {TransientPrice} from "../../src/types/TransientPrice.sol";
import {RequestLock} from "../../src/types/RequestLock.sol";
import {Fulfillment} from "../../src/types/Fulfillment.sol";
import {AssessorJournal} from "../../src/types/AssessorJournal.sol";
import {Offer} from "../../src/types/Offer.sol";
import {Requirements} from "../../src/types/Requirements.sol";
import {Predicate, PredicateType} from "../../src/types/Predicate.sol";
import {Input, InputType} from "../../src/types/Input.sol";
import {IBoundlessMarket} from "../../src/IBoundlessMarket.sol";

Vm constant VM = Vm(0x7109709ECfa91a80626fF3989D68f67F5b1DD12D);
bytes32 constant APP_IMAGE_ID = 0x0000000000000000000000000000000000000000000000000000000000000001;
bytes32 constant SET_BUILDER_IMAGE_ID = 0x0000000000000000000000000000000000000000000000000000000000000002;
bytes32 constant ASSESSOR_IMAGE_ID = 0x0000000000000000000000000000000000000000000000000000000000000003;
bytes constant APP_JOURNAL = bytes("GUEST JOURNAL");

abstract contract BaseClient {
    using SafeCast for uint256;
    using SafeCast for int256;

    int256 public balanceSnapshot = type(int256).max;
    int256 public stakeBalanceSnapshot = type(int256).max;

    string public identifier;

    IBoundlessMarket public boundlessMarket;
    HitPoints public stakeToken;

    constructor() {}

    function initialize(string memory _identifier, IBoundlessMarket _boundlessMarket, HitPoints _stakeToken)
        public
        virtual
    {
        identifier = _identifier;
        boundlessMarket = _boundlessMarket;
        stakeToken = _stakeToken;
        balanceSnapshot = type(int256).max;
    }

    function addr() public view virtual returns (address);

    function sign(ProofRequest calldata req) public virtual returns (bytes memory);

    function defaultOffer() public view returns (Offer memory) {
        return Offer({
            minPrice: 1 ether,
            maxPrice: 2 ether,
            biddingStart: uint64(block.number),
            rampUpPeriod: uint32(10),
            lockTimeout: uint32(100),
            timeout: uint32(100),
            lockStake: 1 ether
        });
    }

    function defaultRequirements() public pure returns (Requirements memory) {
        return Requirements({
            imageId: bytes32(APP_IMAGE_ID),
            predicate: Predicate({predicateType: PredicateType.DigestMatch, data: abi.encode(sha256(APP_JOURNAL))})
        });
    }

    function request(uint32 idx) public virtual returns (ProofRequest memory);

    function request(uint32 idx, Offer memory offer) public virtual returns (ProofRequest memory);

    function snapshotBalance() public {
        balanceSnapshot = boundlessMarket.balanceOf(addr()).toInt256();
    }

    function snapshotStakeBalance() public {
        stakeBalanceSnapshot = boundlessMarket.balanceOfStake(addr()).toInt256();
    }

    function expectBalanceChange(int256 change) public view {
        require(balanceSnapshot != type(int256).max, "balance snapshot is not set");
        int256 newBalance = boundlessMarket.balanceOf(addr()).toInt256();
        console.log("%s balance at block %d: %d", identifier, block.number, newBalance.toUint256());
        int256 expectedBalance = balanceSnapshot + change;
        require(expectedBalance >= 0, "expected balance cannot be less than 0");
        console.log("%s expected balance at block %d: %d", identifier, block.number, expectedBalance.toUint256());
        require(expectedBalance == newBalance, "balance is not equal to expected value");
    }

    function expectStakeBalanceChange(int256 change) public view {
        require(stakeBalanceSnapshot != type(int256).max, "stake balance snapshot is not set");
        int256 newBalance = boundlessMarket.balanceOfStake(addr()).toInt256();
        console.log("%s stake balance at block %d: %d", identifier, block.number, newBalance.toUint256());
        int256 expectedBalance = stakeBalanceSnapshot + change;
        require(expectedBalance >= 0, "expected stake balance cannot be less than 0");
        console.log("%s expected stake balance at block %d: %d", identifier, block.number, expectedBalance.toUint256());
        require(expectedBalance == newBalance, "stake balance is not equal to expected value");
    }
}
