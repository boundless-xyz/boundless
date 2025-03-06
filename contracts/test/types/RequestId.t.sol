// Copyright (c) 2025 RISC Zero, Inc.
//
// All rights reserved.

pragma solidity ^0.8.20;

import "forge-std/Test.sol";
import {RequestId, RequestIdLibrary} from "../../src/types/RequestId.sol";

contract RequestIdTest is Test {
    function testClientAndIndexWithoutFlag() public view {
        address testClient = address(this);
        uint32 testIndex = 1;

        RequestId id = RequestIdLibrary.from(testClient, testIndex);
        (address client, uint32 index, bool isSmartContractSig) = id.clientIndexAndSignatureType();
        assertEq(client, testClient);
        assertEq(index, testIndex);
        assertEq(isSmartContractSig, false);
    }

    function testClientAndIndexWithFlagTrue() public view {
        address testClient = address(this);
        uint32 testIndex = 42;

        RequestId id = RequestIdLibrary.from(testClient, testIndex, true);
        (address client, uint32 index, bool isSmartContractSig) = id.clientIndexAndSignatureType();
        assertEq(client, testClient);
        assertEq(index, testIndex);
        assertEq(isSmartContractSig, true);
    }

    function testClientAndIndexWithFlagFalse() public view {
        address testClient = address(this);
        uint32 testIndex = 100;

        RequestId id = RequestIdLibrary.from(testClient, testIndex, false);
        (address client, uint32 index, bool isSmartContractSig) = id.clientIndexAndSignatureType();
        assertEq(client, testClient);
        assertEq(index, testIndex);
        assertEq(isSmartContractSig, false);
    }

    function testDefaultFlagMatchesExplicitFalse() public view {
        address testClient = address(this);
        uint32 testIndex = 255;

        RequestId id1 = RequestIdLibrary.from(testClient, testIndex);
        RequestId id2 = RequestIdLibrary.from(testClient, testIndex, false);
        
        (address client1, uint32 index1, bool flag1) = id1.clientIndexAndSignatureType();
        (address client2, uint32 index2, bool flag2) = id2.clientIndexAndSignatureType();
        
        assertEq(client1, client2);
        assertEq(index1, index2);
        assertEq(flag1, flag2);
        assertEq(RequestId.unwrap(id1), RequestId.unwrap(id2));
    }
}
