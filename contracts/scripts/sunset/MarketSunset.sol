// Copyright 2026 Boundless Foundation, Inc.
//
// Use of this source code is governed by the Business Source License
// as found in the LICENSE-BSL file.

pragma solidity ^0.8.26;

import {Vm} from "forge-std/Vm.sol";
import {BoundlessMarket} from "boundless-market/BoundlessMarket.sol";
import {IBoundlessRouter} from "../../src/router/interfaces/IBoundlessRouter.sol";
import {RuntimeCheck} from "./RuntimeCheck.sol";
import {SunsetTransaction} from "./SunsetTransaction.sol";

library MarketSunset {
    Vm private constant VM = Vm(address(uint160(uint256(keccak256("hevm cheat code")))));
    bytes32 private constant SLOT = 0x360894a13ba1a3210667c828492db98dca3e2076cc3735a920a3ca505d382bbc;

    function prepare(address requested)
        internal
        returns (SunsetTransaction.Transaction memory transaction, address candidate)
    {
        address proxy;
        address safe;
        address baseline;
        bytes32 baselineHash;
        if (block.chainid == 8453) {
            proxy = 0xFd152dADc5183870710FE54f939Eae3aB9F0fE82;
            safe = 0xB04D1a222789A76E74168a919b43b20F66e24f0b;
            baseline = 0xBf3b740FaF258F8E4ab214702a96918ab739a686;
            baselineHash = 0x857c1fb15b2673b0434ea6ce56800e7fda921baa1e29c6fd0f68b4e04bdccda6;
        } else {
            require(block.chainid == 167000, "unsupported market chain");
            proxy = 0xb3f5C7B4379052EADE8C7f3fa6da37Fb871DA28B;
            safe = 0x8E537AAeddbC98a9a095397aDb7c0d738458Cc8b;
            baseline = 0x0824b650350CC2F510aa22e061a917524945b78c;
            baselineHash = 0x894769a576b0342d1a34c8a38db498320e054dab89ca9ac55d013de95331a0b7;
        }
        require(
            address(uint160(uint256(VM.load(proxy, SLOT)))) == baseline && baseline.codehash == baselineHash,
            "unexpected market baseline"
        );
        BoundlessMarket market = BoundlessMarket(payable(proxy));
        require(market.hasRole(market.ADMIN_ROLE(), safe), "unexpected market admin");
        // Check before CREATE, so its predicted address cannot masquerade as a deployed candidate.
        if (requested != address(0)) RuntimeCheck.requireExisting(requested);
        address local = address(
            new BoundlessMarket(
                IBoundlessRouter(address(market.ROUTER())), market.COLLATERAL_TOKEN_CONTRACT(), market.LEGACY_IMPL()
            )
        );
        candidate = requested == address(0) ? local : requested;
        RuntimeCheck.check(
            local,
            candidate,
            string.concat(
                VM.projectRoot(), "/", VM.envString("SUNSET_ARTIFACT_ROOT"), "/BoundlessMarket.sol/BoundlessMarket.json"
            )
        );
        return (SunsetTransaction.single(safe, proxy, candidate), candidate);
    }
}
