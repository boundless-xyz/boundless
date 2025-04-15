// Copyright (c) 2025 RISC Zero, Inc.
//
// All rights reserved.
pragma solidity ^0.8.24;

import {IERC1271} from "@openzeppelin/contracts/interfaces/IERC1271.sol";
import {IBoundlessMarket} from "../../src/IBoundlessMarket.sol";
import {IERC20} from "@openzeppelin/contracts/token/ERC20/IERC20.sol";

/// @dev Simple mock implementation of an ERC-1271 compliant SCW.
contract MockSmartContractWallet is IERC1271 {
    bytes private expectedSignature;
    uint256 private gasCost = 0;
    address private owner;
    IBoundlessMarket public immutable market;
    bytes4 internal constant MAGICVALUE = 0x1626ba7e; // bytes4(keccak256("isValidSignature(bytes32,bytes)")

    constructor(bytes memory _expectedSignature, IBoundlessMarket _market, address _owner) {
        expectedSignature = _expectedSignature;
        market = _market;
        owner = _owner;
    }

    function setExpectedSignature(bytes memory _expectedSignature) external {
        expectedSignature = _expectedSignature;
    }

    function setGasCost(uint256 _gasCost) external {
        gasCost = _gasCost;
    }

    function isValidSignature(bytes32, bytes memory _signature) external view returns (bytes4) {
        // Consume gas by doing SLOAD operations to random slots
        uint256 startGas = gasleft();
        uint256 i = 0;
        while (startGas - gasleft() < gasCost) {
            bytes32 slot = keccak256(abi.encode(i));
            bytes32 x;
            assembly {
                x := sload(slot)
            }
            i++;
        }

        if (keccak256(_signature) == keccak256(expectedSignature)) {
            return MAGICVALUE;
        }
        return 0xffffffff;
    }

    // Allow the wallet to receive ETH
    receive() external payable {}

    function execute(address target, bytes memory data, uint256 value) external payable {
        require(msg.sender == owner, "Not authorized");
        (bool success,) = target.call{value: value}(data);
        require(success, "Call failed");
    }
}
