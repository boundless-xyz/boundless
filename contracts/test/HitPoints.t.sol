// Copyright (c) 2025 RISC Zero, Inc.
//
// All rights reserved.

pragma solidity ^0.8.24;

import "forge-std/Test.sol";
import "@openzeppelin/contracts/access/Ownable.sol";
import "../src/HitPoints.sol";

contract HitPointsTest is Test {
    HitPoints public token;
    address public owner;
    address public authorized;
    address public user;

    function setUp() public {
        owner = address(this);
        authorized = makeAddr("authorized");
        user = makeAddr("user");

        token = new HitPoints(owner);
    }

    function testInitialState() public view {
        assertEq(token.name(), "HitPoints");
        assertEq(token.symbol(), "HP");
        assertEq(token.decimals(), 18);
        assertEq(token.owner(), owner);
        assertTrue(token.isAuthorized(owner));
        assertFalse(token.isAuthorized(authorized));
    }

    function testAuthorization() public {
        token.authorize(authorized);
        assertTrue(token.isAuthorized(authorized));

        token.deauthorize(authorized);
        assertFalse(token.isAuthorized(authorized));
    }

    function testAuthorizationRevertNotOwner() public {
        vm.prank(user);
        vm.expectRevert(abi.encodeWithSelector(Ownable.OwnableUnauthorizedAccount.selector, user));
        token.authorize(authorized);
    }

    function testMint() public {
        uint256 initialSupply = token.totalSupply();
        token.mint(user, 100);
        assertEq(token.balanceOf(user), 100);
        assertEq(token.totalSupply(), initialSupply + 100);
    }

    function testMintRevertUnauthorized() public {
        vm.prank(user);
        vm.expectRevert(abi.encodeWithSignature("Unauthorized()"));
        token.mint(user, 100);
    }

    function testTransferToAuthorizedRecipient() public {
        token.mint(user, 100);
        token.authorize(authorized);

        vm.prank(user);
        token.transfer(authorized, 50);
        assertEq(token.balanceOf(user), 50);
        assertEq(token.balanceOf(authorized), 50);
    }

    function testTransferFromAuthorizedRecipient() public {
        token.mint(authorized, 100);
        token.authorize(authorized);

        vm.prank(authorized);
        token.transfer(user, 50);
        assertEq(token.balanceOf(authorized), 50);
        assertEq(token.balanceOf(user), 50);
    }

    function testTransferRevertUnauthorizedRecipient() public {
        token.mint(user, 100);

        vm.prank(user);
        vm.expectRevert(abi.encodeWithSignature("UnauthorizedTransfer()"));
        token.transfer(authorized, 50);
    }

    function testApproveAndTransferFrom() public {
        token.mint(user, 100);
        token.authorize(authorized);

        vm.prank(user);
        token.approve(authorized, 50);

        vm.prank(authorized);
        token.transferFrom(user, authorized, 50);

        assertEq(token.balanceOf(user), 50);
        assertEq(token.balanceOf(authorized), 50);
    }

    function testTransferFromRevertUnauthorizedRecipient() public {
        token.mint(user, 100);

        vm.prank(user);
        token.approve(authorized, 50);

        vm.prank(authorized);
        vm.expectRevert(abi.encodeWithSignature("UnauthorizedTransfer()"));
        token.transferFrom(user, authorized, 50);
    }

    function testFuzzMint(address _user, uint256 _amount) public {
        vm.assume(_user != address(0));
        vm.assume(_amount < type(uint256).max);

        uint256 initialSupply = token.totalSupply();
        token.mint(_user, _amount);
        assertEq(token.balanceOf(_user), _amount);
        assertEq(token.totalSupply(), initialSupply + _amount);
    }

    function testFuzzTransfer(address _from, uint256 _amount) public {
        vm.assume(_from != address(0));
        vm.assume(_amount < type(uint256).max);

        token.authorize(authorized);
        token.mint(_from, _amount);

        vm.prank(_from);
        token.transfer(authorized, _amount);

        assertEq(token.balanceOf(_from), 0);
        assertEq(token.balanceOf(authorized), _amount);
    }
}
