// Copyright 2026 Boundless Foundation, Inc.
//
// Use of this source code is governed by the Business Source License
// as found in the LICENSE-BSL file.
pragma solidity ^0.8.26;

import {IBoundlessMarket} from "../../src/IBoundlessMarket.sol";
import {HitPoints} from "../../src/HitPoints.sol";
import {Vm} from "forge-std/Test.sol";
import {console} from "forge-std/console.sol";
import {SafeCast} from "@openzeppelin/contracts/utils/math/SafeCast.sol";
import {Callback} from "../../src/types/Callback.sol";
import {ProofRequest} from "../../src/types/ProofRequest.sol";
import {LockRequest} from "../../src/types/LockRequest.sol";
import {Offer} from "../../src/types/Offer.sol";
import {Requirements} from "../../src/types/Requirements.sol";
import {PredicateLibrary} from "../../src/types/Predicate.sol";

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
    HitPoints public collateralToken;

    constructor() {}

    function initialize(string memory _identifier, IBoundlessMarket _boundlessMarket, HitPoints _collateralToken)
        public
        virtual
    {
        identifier = _identifier;
        boundlessMarket = _boundlessMarket;
        collateralToken = _collateralToken;
        balanceSnapshot = type(int256).max;
    }

    function addr() public view virtual returns (address);

    function sign(ProofRequest calldata req) public virtual returns (bytes memory);

    function signLockRequest(LockRequest calldata req) public virtual returns (bytes memory);

    function defaultOffer() public view returns (Offer memory) {
        return Offer({
            minPrice: 1 ether,
            maxPrice: 2 ether,
            rampUpStart: uint64(block.timestamp),
            rampUpPeriod: uint32(10),
            lockTimeout: uint32(100),
            timeout: uint32(200),
            lockCollateral: 1 ether
        });
    }

    function defaultRequirements() public pure returns (Requirements memory) {
        return Requirements({
            predicate: PredicateLibrary.createDigestMatchPredicate(bytes32(APP_IMAGE_ID), sha256(APP_JOURNAL)),
            selector: bytes4(0),
            callback: Callback({addr: address(0), gasLimit: 0})
        });
    }

    function request(uint32 idx) public virtual returns (ProofRequest memory);

    function request(uint32 idx, Offer memory offer) public virtual returns (ProofRequest memory);

    function snapshotBalance() public {
        balanceSnapshot = boundlessMarket.balanceOf(addr()).toInt256();
    }

    function snapshotCollateralBalance() public {
        stakeBalanceSnapshot = boundlessMarket.balanceOfCollateral(addr()).toInt256();
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

    function expectCollateralBalanceChange(int256 change) public view {
        require(stakeBalanceSnapshot != type(int256).max, "collateral balance snapshot is not set");
        int256 newBalance = boundlessMarket.balanceOfCollateral(addr()).toInt256();
        console.log("%s collateral balance at block %d: %d", identifier, block.number, newBalance.toUint256());
        int256 expectedBalance = stakeBalanceSnapshot + change;
        require(expectedBalance >= 0, "expected collateral balance cannot be less than 0");
        console.log(
            "%s expected collateral balance at block %d: %d", identifier, block.number, expectedBalance.toUint256()
        );
        require(expectedBalance == newBalance, "collateral balance is not equal to expected value");
    }
}
