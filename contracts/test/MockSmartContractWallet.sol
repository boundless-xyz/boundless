// SPDX-License-Identifier: BUSL-1.1
pragma solidity ^0.8.24;

import {IERC1271} from "@openzeppelin/contracts/interfaces/IERC1271.sol";
import {IBoundlessMarket} from "../src/IBoundlessMarket.sol";
import {IERC20} from "@openzeppelin/contracts/token/ERC20/IERC20.sol";

contract MockSmartContractWallet is IERC1271 {
    bytes private expectedSignature;
    IBoundlessMarket public immutable market;
    bytes4 constant internal MAGICVALUE = 0x1626ba7e; // bytes4(keccak256("isValidSignature(bytes32,bytes)")

    constructor(bytes memory _expectedSignature, IBoundlessMarket _market) {
        expectedSignature = _expectedSignature;
        market = _market;
    }

    function isValidSignature(bytes32, bytes memory _signature) external view returns (bytes4) {
        // Simple check - does the signature match what we expect
        if (keccak256(_signature) == keccak256(expectedSignature)) {
            return MAGICVALUE;
        }
        return 0xffffffff;
    }

    // Allow the wallet to receive ETH
    receive() external payable {}

    function deposit() external payable {
        market.deposit{value: msg.value}();
    }

    function depositStake(IERC20 stakeToken, uint256 amount) external {
        // Approve the market to spend tokens
        stakeToken.approve(address(market), amount);
        // Deposit stake
        market.depositStake(amount);
    }
} 