// Copyright 2025 RISC Zero, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

pragma solidity ^0.8.20;

import {RiscZeroCheats} from "risc0/test/RiscZeroCheats.sol";
import {Receipt as RiscZeroReceipt, IRiscZeroVerifier} from "risc0/IRiscZeroVerifier.sol";
import {RiscZeroMockVerifier} from "risc0/test/RiscZeroMockVerifier.sol";
import {console2} from "forge-std/console2.sol";
import {Test} from "forge-std/Test.sol";
import {Counter} from "../src/Counter.sol";
import {Elf} from "boundless-market/../test/UtilElf.sol"; // auto-generated contract after running `cargo build`.
import {ImageID} from "boundless-market/libraries/UtilImageID.sol"; // auto-generated contract after running `cargo build`.

contract CounterTest is RiscZeroCheats, Test {
    Counter public counter;
    RiscZeroMockVerifier public verifier;

    bytes public constant MOCK_JOURNAL = bytes("I'm the journal for _some_ zkVM program");
    bytes32 public constant IMAGE_ID = bytes32(uint256(0xec80));

    function setUp() public {
        verifier = new RiscZeroMockVerifier(0);
        counter = new Counter(verifier);
        assertEq(counter.count(address(this)), 0);
    }

    function test_Increment() public {
        RiscZeroReceipt memory receipt = verifier.mockProve(IMAGE_ID, sha256(MOCK_JOURNAL));

        counter.increment(receipt.seal, IMAGE_ID, sha256(MOCK_JOURNAL));
        assertEq(counter.count(address(this)), 1);
    }
}
