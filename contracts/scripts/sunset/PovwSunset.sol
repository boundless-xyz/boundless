// Copyright 2026 Boundless Foundation, Inc.
//
// Use of this source code is governed by the Business Source License
// as found in the LICENSE-BSL file.

pragma solidity ^0.8.26;

import {Vm} from "forge-std/Vm.sol";
import {PovwAccounting} from "../../src/povw/PovwAccounting.sol";
import {PovwMint} from "../../src/povw/PovwMint.sol";
import {RuntimeCheck} from "./RuntimeCheck.sol";
import {SunsetTransaction} from "./SunsetTransaction.sol";

library PovwSunset {
    Vm private constant VM = Vm(address(uint160(uint256(keccak256("hevm cheat code")))));
    address internal constant SAFE = 0x3886eEaf95AA2bDDdf0C924925e290291f70447F;
    address internal constant MULTISEND = 0x40A2aCCbd92BCA938b02010E17A5b8929b49130D;
    address internal constant ACCOUNTING = 0x319bd4050b2170a7aE3Ead3E6d5AB8a5c7cFBDF8;
    address internal constant MINT = 0xBFCE7c2d5e7EdDEab71B3eeED770713c8b755397;
    bytes32 private constant SLOT = 0x360894a13ba1a3210667c828492db98dca3e2076cc3735a920a3ca505d382bbc;

    function prepare(address requestedAccounting, address requestedMint)
        internal
        returns (SunsetTransaction.Transaction memory transaction, address candidateAccounting, address candidateMint)
    {
        require(block.chainid == 1, "Ethereum required");
        address a = address(uint160(uint256(VM.load(ACCOUNTING, SLOT))));
        address m = address(uint160(uint256(VM.load(MINT, SLOT))));
        require(
            a == 0xfb2187436fD8e266E4F18cBCe9aAFbDbB796e67C
                && a.codehash == 0x4bac90ef98d198c558b236c0004335cbbd71f341623eb723679f424ae23654d9,
            "unexpected accounting baseline"
        );
        require(
            m == 0x569D48052d59dC4aAC86098fE8db43D0E514f59B
                && m.codehash == 0x45300b4466837e64abce4d91f75fd4576438490a5430ba938a892f2dab816cc8,
            "unexpected mint baseline"
        );
        PovwAccounting accounting = PovwAccounting(ACCOUNTING);
        PovwMint minter = PovwMint(MINT);
        require(accounting.owner() == SAFE && minter.owner() == SAFE, "unexpected PoVW owner");
        require(
            MULTISEND.codehash == 0xa9865ac2d9c7a1591619b188c4d88167b50df6cc0c5327fcbd1c8c75f7c066ad,
            "unexpected MultiSend runtime"
        );
        // Supplied implementations must exist before local reference deployments.
        if (requestedAccounting != address(0)) RuntimeCheck.requireExisting(requestedAccounting);
        if (requestedMint != address(0)) RuntimeCheck.requireExisting(requestedMint);
        address localAccounting =
            address(new PovwAccounting(accounting.VERIFIER(), accounting.TOKEN(), accounting.LOG_UPDATER_ID()));
        address localMint = address(
            new PovwMint(
                minter.VERIFIER(), accounting, minter.MINT_CALCULATOR_ID(), minter.TOKEN(), minter.TOKEN_REWARDS()
            )
        );
        candidateAccounting = requestedAccounting == address(0) ? localAccounting : requestedAccounting;
        candidateMint = requestedMint == address(0) ? localMint : requestedMint;
        string memory folder = string.concat(VM.projectRoot(), "/", VM.envString("SUNSET_ARTIFACT_ROOT"));
        RuntimeCheck.check(
            localAccounting, candidateAccounting, string.concat(folder, "/PovwAccounting.sol/PovwAccounting.json")
        );
        RuntimeCheck.check(localMint, candidateMint, string.concat(folder, "/PovwMint.sol/PovwMint.json"));
        return (
            SunsetTransaction.pair(SAFE, MULTISEND, ACCOUNTING, candidateAccounting, MINT, candidateMint),
            candidateAccounting,
            candidateMint
        );
    }
}
