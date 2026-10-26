// Copyright 2026 Boundless Foundation, Inc.
//
// Use of this source code is governed by the Business Source License
// as found in the LICENSE-BSL file.

pragma solidity ^0.8.26;
import {SunsetPrinter} from "./SunsetPrinter.sol";
import {SunsetTransaction} from "./SunsetTransaction.sol";
import {MarketSunset} from "./MarketSunset.sol";

contract PrintMarketSunset is SunsetPrinter {
    function run() public {
        require(vm.envAddress("SUNSET_IMPL") != address(0), "SUNSET_IMPL required");
        (SunsetTransaction.Transaction memory transaction,) = MarketSunset.prepare(vm.envAddress("SUNSET_IMPL"));
        printTransaction(transaction);
    }
}
