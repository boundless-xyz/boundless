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
        (uint256 available, uint256 locked) = token.balanceOf(user);
        assertEq(available, uint256(100), "Invalid available balance after mint");
        assertEq(locked, uint256(0), "Invalid locked balance after mint");
        assertEq(token.totalSupply(), uint256(100), "Invalid total supply after mint");
    }

    function testMintRevertNotOwner() public {
        vm.prank(user);
        vm.expectRevert(abi.encodeWithSignature("OwnableUnauthorizedAccount(address)", user));
        token.mint(user, 100);
    }

    // Locking tests
    function testLock() public {
        token.authorize(authorizedAddress);
        token.mint(user, 1000);

        vm.prank(authorizedAddress);
        token.lock(user, 300);

        (uint256 available, uint256 locked) = token.balanceOf(user);
        assertEq(uint256(available), uint256(700), "Invalid available balance after lock");
        assertEq(uint256(locked), uint256(300), "Invalid locked balance after lock");
        assertEq(token.totalSupply(), uint256(1000), "Invalid total supply after lock");
    }

    function testLockRevertInsufficientBalance() public {
        token.authorize(authorizedAddress);
        token.mint(user, 100);

        vm.prank(authorizedAddress);
        vm.expectRevert(abi.encodeWithSignature("InsufficientBalance()"));
        token.lock(user, 200);
    }

    function testLockRevertUnauthorized() public {
        token.mint(user, 1000);

        vm.prank(userTwo);
        vm.expectRevert(abi.encodeWithSignature("UnauthorizedCaller()"));
        token.lock(user, 300);
    }

    // Unlocking tests
    function testUnlock() public {
        token.authorize(authorizedAddress);
        token.mint(user, 1000);

        vm.startPrank(authorizedAddress);
        token.lock(user, 300);
        token.unlock(user, 100);
        vm.stopPrank();

        (uint256 available, uint256 locked) = token.balanceOf(user);
        assertEq(uint256(available), uint256(800), "Invalid available balance after unlock");
        assertEq(uint256(locked), uint256(200), "Invalid locked balance after unlock");
        assertEq(token.totalSupply(), uint256(1000), "Invalid total supply after unlock");
    }

    function testUnlockRevertInsufficientLockedBalance() public {
        token.authorize(authorizedAddress);
        token.mint(user, 1000);

        vm.prank(authorizedAddress);
        token.lock(user, 100);

        vm.prank(authorizedAddress);
        vm.expectRevert(abi.encodeWithSignature("InsufficientLockedBalance()"));
        token.unlock(user, 200);
    }

    function testUnlockRevertUnauthorized() public {
        token.authorize(authorizedAddress);
        token.mint(user, 1000);

        vm.prank(authorizedAddress);
        token.lock(user, 300);

        vm.prank(userTwo);
        vm.expectRevert(abi.encodeWithSignature("UnauthorizedCaller()"));
        token.unlock(user, 100);
    }

    // Burning tests
    function testBurn() public {
        token.authorize(authorizedAddress);
        token.mint(user, 1000);

        vm.prank(authorizedAddress);
        token.lock(user, 300);

        vm.prank(authorizedAddress);
        token.burn(user, 100);

        (uint256 available, uint256 locked) = token.balanceOf(user);
        assertEq(uint256(available), uint256(700), "Invalid available balance after burn");
        assertEq(uint256(locked), uint256(200), "Invalid locked balance after burn");
        assertEq(token.totalSupply(), uint256(900), "Invalid total supply after burn");
    }

    function testBurnRevertInsufficientLockedBalance() public {
        token.authorize(authorizedAddress);
        token.mint(user, 1000);

        vm.prank(authorizedAddress);
        token.lock(user, 100);

        vm.expectRevert(abi.encodeWithSignature("InsufficientLockedBalance()"));
        vm.prank(authorizedAddress);
        token.burn(user, 200);
    }

    function testBurnRevertNotAuthorized() public {
        token.authorize(authorizedAddress);
        token.mint(user, 1000);

        vm.prank(authorizedAddress);
        token.lock(user, 300);

        vm.prank(userTwo);
        vm.expectRevert(abi.encodeWithSignature("UnauthorizedCaller()"));
        token.burn(user, 100);
    }

    // Fuzz tests
    function testFuzzMint(address _user, uint128 _amount) public {
        vm.assume(_user != address(0));

        token.mint(_user, _amount);
        (uint256 available, uint256 locked) = token.balanceOf(_user);
        assertEq(available, uint256(_amount), "Invalid available balance in fuzz test");
        assertEq(locked, uint256(0), "Invalid locked balance in fuzz test");
        assertEq(token.totalSupply(), uint256(_amount), "Invalid total supply in fuzz test");
    }

    function testFuzzLockAndUnlock(address _user, uint128 _initial, uint128 _lockAmount, uint128 _unlockAmount)
        public
    {
        vm.assume(_user != address(0));
        vm.assume(_lockAmount <= _initial);
        vm.assume(_unlockAmount <= _lockAmount);

        token.authorize(authorizedAddress);
        token.mint(_user, _initial);

        vm.startPrank(authorizedAddress);
        token.lock(_user, _lockAmount);
        token.unlock(_user, _unlockAmount);
        vm.stopPrank();

        (uint256 available, uint256 locked) = token.balanceOf(_user);
        assertEq(available, _initial - _lockAmount + _unlockAmount, "Invalid available balance in fuzz lock/unlock");
        assertEq(locked, _lockAmount - _unlockAmount, "Invalid locked balance in fuzz lock/unlock");
        assertEq(token.totalSupply(), _initial, "Invalid total supply in fuzz lock/unlock");
    }
}
