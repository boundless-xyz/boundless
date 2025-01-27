// Copyright (c) 2025 RISC Zero, Inc.
//
// All rights reserved.

pragma solidity ^0.8.20;

import "forge-std/Test.sol";
import {Input, InputLibrary, InputType} from "../../src/types/Input.sol";

contract InputTest is Test {
    function testCreateInlineInput() public pure {
        bytes memory data = "inline data";
        Input memory input = InputLibrary.createInlineInput(data);

        assertEq(uint8(input.inputType), uint8(InputType.Inline), "Input type should be Inline");
        assertEq(input.data, data, "Input data should match");
    }

    function testCreateUrlInput() public pure {
        string memory url = "https://example.com";
        Input memory input = InputLibrary.createUrlInput(url);

        assertEq(uint8(input.inputType), uint8(InputType.Url), "Input type should be Url");
        assertEq(string(input.data), url, "Input data should match the URL");
    }
}
