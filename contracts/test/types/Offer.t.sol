// Copyright (c) 2025 RISC Zero, Inc.
//
// All rights reserved.

pragma solidity ^0.8.20;

import "forge-std/Test.sol";
import {ReceiptClaim, ReceiptClaimLib} from "risc0/IRiscZeroVerifier.sol";
import {RiscZeroMockVerifier} from "risc0/test/RiscZeroMockVerifier.sol";
import {IBoundlessMarket} from "../../src/IBoundlessMarket.sol";
import {Offer} from "../../src/types/Offer.sol";
import {RequestId, RequestIdLibrary} from "../../src/types/RequestId.sol";

contract OfferTest is Test {
    /// forge-config: default.allow_internal_expect_revert = true
    function testBlockAtPrice() public {
        Offer memory offer = Offer({
            minPrice: 1 ether,
            maxPrice: 2 ether,
            biddingStart: uint64(100),
            rampUpPeriod: 100,
            lockTimeout: uint32(500),
            timeout: uint32(500),
            lockStake: 0.1 ether
        });

        assertEq(offer.blockAtPrice(1 ether), 0);

        assertEq(offer.blockAtPrice(1.01 ether), 101);
        assertEq(offer.blockAtPrice(1.001 ether), 101);

        assertEq(offer.blockAtPrice(1.25 ether), 125);
        assertEq(offer.blockAtPrice(1.5 ether), 150);
        assertEq(offer.blockAtPrice(1.75 ether), 175);
        assertEq(offer.blockAtPrice(1.99 ether), 199);

        assertEq(offer.blockAtPrice(2 ether), 200);

        vm.expectRevert(abi.encodeWithSelector(IBoundlessMarket.InvalidRequest.selector));
        offer.blockAtPrice(3 ether);
    }

    function testPriceAtBlock() public pure {
        Offer memory offer = Offer({
            minPrice: 1 ether,
            maxPrice: 2 ether,
            biddingStart: uint64(100),
            rampUpPeriod: 100,
            lockTimeout: uint32(500),
            timeout: uint32(500),
            lockStake: 0.1 ether
        });

        assertEq(offer.priceAtBlock(0), 1 ether);
        assertEq(offer.priceAtBlock(100), 1 ether);

        assertEq(offer.priceAtBlock(101), 1.01 ether);
        assertEq(offer.priceAtBlock(125), 1.25 ether);
        assertEq(offer.priceAtBlock(150), 1.5 ether);
        assertEq(offer.priceAtBlock(175), 1.75 ether);
        assertEq(offer.priceAtBlock(199), 1.99 ether);

        assertEq(offer.priceAtBlock(200), 2 ether);
        assertEq(offer.priceAtBlock(500), 2 ether);
    }

    function testDeadlines() public pure {
        Offer memory offer = Offer({
            minPrice: 1 ether,
            maxPrice: 2 ether,
            biddingStart: uint64(100),
            rampUpPeriod: 100,
            lockTimeout: uint32(150),
            timeout: uint32(200),
            lockStake: 0.1 ether
        });

        assertEq(offer.lockDeadline(), 250);
        assertEq(offer.deadline(), 300);
    }

    /// forge-config: default.allow_internal_expect_revert = true
    function testInvalidLockTimeout() public {
        Offer memory invalidOffer = Offer({
            minPrice: 1 ether,
            maxPrice: 2 ether,
            biddingStart: uint64(100),
            rampUpPeriod: 100,
            lockTimeout: uint32(250),
            timeout: uint32(200),
            lockStake: 0.1 ether
        });

        RequestId id = RequestIdLibrary.from(address(this), 1);
        vm.expectRevert(abi.encodeWithSelector(IBoundlessMarket.InvalidRequest.selector));
        invalidOffer.validate(id);
    }

    /// forge-config: default.allow_internal_expect_revert = true
    function testDeadlineDeltaTooLarge() public {
        Offer memory invalidOffer = Offer({
            minPrice: 1 ether,
            maxPrice: 2 ether,
            biddingStart: uint64(100),
            rampUpPeriod: 100,
            lockTimeout: uint32(500),
            timeout: uint32(uint32(500) + type(uint24).max + 1), // Makes deadline - lockDeadline > type(uint24).max
            lockStake: 0.1 ether
        });

        RequestId id = RequestIdLibrary.from(address(this), 1);
        vm.expectRevert(abi.encodeWithSelector(IBoundlessMarket.InvalidRequest.selector));
        invalidOffer.validate(id);
    }

    function testValidTimeouts() public view {
        Offer memory validOffer = Offer({
            minPrice: 1 ether,
            maxPrice: 2 ether,
            biddingStart: uint64(100),
            rampUpPeriod: 100,
            lockTimeout: uint32(500),
            timeout: uint32(uint32(500) + type(uint24).max), // Maximum valid difference
            lockStake: 0.1 ether
        });

        RequestId id = RequestIdLibrary.from(address(this), 1);
        (uint64 lockDeadline, uint64 deadline) = validOffer.validate(id);

        assertEq(lockDeadline, 600); // biddingStart + lockTimeout
        assertEq(deadline, uint32(600) + type(uint24).max); // biddingStart + timeout
        assertEq(deadline - lockDeadline, type(uint24).max); // Maximum allowed difference
    }
}
