// Copyright (c) 2025 RISC Zero, Inc.
//
// All rights reserved.

pragma solidity ^0.8.24;

import {Test} from "forge-std/Test.sol";
import {HitPoints} from "../src/HitPoints.sol";
import {IHitPoints} from "../src/IHitPoints.sol";

contract HitPointsTest is Test {
    HitPoints public token;
    address public owner;
    address public authorizedAddress;
    address public user;
    address public userTwo;

    function setUp() public {
        owner = address(this);
        authorizedAddress = makeAddr("authorized");
        user = makeAddr("user");
        userTwo = makeAddr("userTwo");

        token = new HitPoints(owner);
    }

    // Authorization tests
    function testAuthorize() public {
        token.authorize(authorizedAddress);
        assertTrue(token.isAuthorized(authorizedAddress), "Should be authorized");
    }

    function testAuthorizeRevertNotOwner() public {
        vm.prank(user);
        vm.expectRevert(abi.encodeWithSignature("OwnableUnauthorizedAccount(address)", user));
        token.authorize(authorizedAddress);
    }

    function testDeauthorize() public {
        token.authorize(authorizedAddress);
        token.deauthorize(authorizedAddress);
        assertFalse(token.isAuthorized(authorizedAddress), "Should not be authorized");
    }

    // Minting tests
    function testMint() public {
        token.mint(user, 100);
        assertEq(token.balanceOf(user), uint256(100), "Invalid available balance after mint");
    }

    function testMintRevertNotAuthorized() public {
        vm.prank(user);
        vm.expectRevert(abi.encodeWithSignature("UnauthorizedCaller()"));
        token.mint(user, 100);
    }

    // Burning tests
    function testBurn() public {
        token.authorize(authorizedAddress);
        token.mint(user, 1000);

        vm.prank(authorizedAddress);
        token.burn(user, 100);

        assertEq(token.balanceOf(user), uint256(900), "Invalid available balance after burn");
    }

    function testBurnRevertInsufficientBalance() public {
        token.authorize(authorizedAddress);
        token.mint(user, 100);

        vm.expectRevert(abi.encodeWithSelector(IHitPoints.InsufficientBalance.selector, user));
        vm.prank(authorizedAddress);
        token.burn(user, 200);
    }

    function testBurnRevertNotAuthorized() public {
        token.authorize(authorizedAddress);
        token.mint(user, 1000);

        vm.prank(userTwo);
        vm.expectRevert(abi.encodeWithSignature("UnauthorizedCaller()"));
        token.burn(user, 100);
    }
}
