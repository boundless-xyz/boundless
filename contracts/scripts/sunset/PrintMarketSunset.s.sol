// SPDX-License-Identifier: MIT
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
