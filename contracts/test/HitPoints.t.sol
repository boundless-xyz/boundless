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
        assertTrue(token.isAuthorized(address(0)));
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
        vm.expectRevert(abi.encodeWithSelector(Ownable.OwnableUnauthorizedAccount.selector, user));
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
        vm.expectRevert(abi.encodeWithSelector(IHitPoints.UnauthorizedTransfer.selector));
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
        vm.expectRevert(abi.encodeWithSelector(IHitPoints.UnauthorizedTransfer.selector));
        token.transferFrom(user, authorized, 50);
    }

    function testFuzzMint(address _user, uint256 _amount) public {
        vm.assume(_user != address(0));
        vm.assume(_amount <= type(uint96).max);

        uint256 initialSupply = token.totalSupply();

        token.mint(_user, _amount);

        assertEq(token.balanceOf(_user), _amount);
        assertEq(token.totalSupply(), initialSupply + _amount);
    }

    function testFuzzMintExceedLimit(address _user, uint256 _existingAmount) public {
        vm.assume(_user != address(0));
        vm.assume(_existingAmount <= type(uint96).max - 1);

        // Mint existing amount
        token.mint(_user, _existingAmount);

        // Calculate mint amount that would exceed limit
        uint256 _mintAmount = type(uint96).max - _existingAmount + 1;

        // Expect revert when minting would exceed uint96 max
        vm.expectRevert(
            abi.encodeWithSelector(IHitPoints.BalanceExceedsLimit.selector, _user, _existingAmount, _mintAmount)
        );
        token.mint(_user, _mintAmount);
    }

    function testFuzzTransfer(address _from, uint256 _amount) public {
        vm.assume(_from != address(0));
        vm.assume(_amount > 0); // Ensure non-zero transfer
        vm.assume(_amount <= type(uint96).max);

        // Create a recipient
        address recipient = makeAddr("recipient");

        // Authorize the sender
        token.authorize(_from);

        // Mint tokens to the sender
        token.mint(_from, _amount);

        // Perform the transfer
        vm.prank(_from);
        token.transfer(recipient, _amount);

        // Check balances
        assertEq(token.balanceOf(_from), 0, "Sender balance should be zero");
        assertEq(token.balanceOf(recipient), _amount, "Recipient balance should match transferred amount");
    }

    function testFuzzTransferExceedLimit(address _recipient) public {
        vm.assume(_recipient != address(0));

        address _sender = makeAddr("sender");

        // Ensure authorized
        token.authorize(_recipient);
        token.authorize(_sender);

        // Amount that would almost max out uint96
        uint256 existingBalance = type(uint96).max - 1;

        // Mint to recipient
        token.mint(_recipient, existingBalance);

        // Mint a transfer amount to sender
        uint256 transferAmount = 2;
        token.mint(_sender, transferAmount);

        // Expect revert when transfer would exceed uint96 max
        vm.prank(_sender);
        vm.expectRevert(
            abi.encodeWithSelector(IHitPoints.BalanceExceedsLimit.selector, _recipient, existingBalance, transferAmount)
        );
        token.transfer(_recipient, transferAmount);
    }
}
