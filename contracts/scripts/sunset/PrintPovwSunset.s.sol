// Copyright 2026 Boundless Foundation, Inc.
//
// Use of this source code is governed by the Business Source License
// as found in the LICENSE-BSL file.

pragma solidity ^0.8.26;
import {SunsetPrinter} from "./SunsetPrinter.sol";
import {SunsetTransaction} from "./SunsetTransaction.sol";
import {PovwSunset} from "./PovwSunset.sol";

contract PrintPovwSunset is SunsetPrinter {
    function run() public {
        require(
            vm.envAddress("ACCOUNTING_SUNSET_IMPL") != address(0) && vm.envAddress("MINT_SUNSET_IMPL") != address(0),
            "both PoVW implementations required"
        );
        (SunsetTransaction.Transaction memory transaction,,) =
            PovwSunset.prepare(vm.envAddress("ACCOUNTING_SUNSET_IMPL"), vm.envAddress("MINT_SUNSET_IMPL"));
        printTransaction(transaction);
    }
}
