// Copyright (c) 2025 RISC Zero Inc,
//
// Use of this source code is governed by the Business Source License
// as found in the LICENSE-BSL file.

pragma solidity ^0.8.20;

import "forge-std/Test.sol";
import {RequestId, RequestIdLibrary} from "../../src/types/RequestId.sol";

contract RequestIdTest is Test {
    function testClientAndIndex() public view {
        address testClient = address(this);
        uint32 testIndex = 1;

        RequestId id = RequestIdLibrary.from(testClient, testIndex);
        (address client, uint32 index) = id.clientAndIndex();
        assertEq(client, testClient, "Client address should match the original address");
        assertEq(index, testIndex, "Index should match the original index");
    }
}
