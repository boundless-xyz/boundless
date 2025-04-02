// Copyright (c) 2025 RISC Zero, Inc.
//
// All rights reserved.
pragma solidity ^0.8.13;

import {IBoundlessMarket} from "../../src/IBoundlessMarket.sol";
import {HitPoints} from "../../src/HitPoints.sol";
import {BaseClient} from "./BaseClient.sol";
import {MessageHashUtils} from "@openzeppelin/contracts/utils/cryptography/MessageHashUtils.sol";
import {ERC20Permit} from "@openzeppelin/contracts/token/ERC20/extensions/ERC20Permit.sol";
import {TestUtils} from "../TestUtils.sol";
import "forge-std/Test.sol";
import {MockSmartContractWallet} from "./MockSmartContractWallet.sol";
import "forge-std/Vm.sol";
import {ProofRequest} from "../../src/types/ProofRequest.sol";
import {LockRequest} from "../../src/types/LockRequest.sol";
import {RequestIdLibrary} from "../../src/types/RequestId.sol";
import {Input, InputType} from "../../src/types/Input.sol";
import {Offer} from "../../src/types/Offer.sol";

Vm constant VM = Vm(0x7109709ECfa91a80626fF3989D68f67F5b1DD12D);

/// @dev SmartContractClient is essentially a wrapper around a smart contract wallet with logic for signing proof requests and submitting them to the market.
/// It also inherits functions for tracking balances and stake from BaseClient.
contract SmartContractClient is BaseClient, Test {
    MockSmartContractWallet public smartWallet;
    Vm.Wallet public signer;

    bytes private expectedSignature;

    constructor(Vm.Wallet memory _signer) BaseClient() {
        expectedSignature = abi.encodePacked(keccak256(abi.encodePacked(_signer.addr)));
        smartWallet = new MockSmartContractWallet(expectedSignature, boundlessMarket, _signer.addr);
        signer = _signer;
    }

    function initialize(string memory _identifier, IBoundlessMarket _boundlessMarket, HitPoints _stakeToken)
        public
        override
    {
        vm.label(address(smartWallet), _identifier);
        super.initialize(_identifier, _boundlessMarket, _stakeToken);
    }

    function addr() public view override returns (address) {
        return address(smartWallet);
    }

    function signerAddr() public view returns (address) {
        return signer.addr;
    }

    function request(uint32 idx) public view override returns (ProofRequest memory) {
        return ProofRequest({
            id: RequestIdLibrary.from(addr(), idx, true),
            requirements: defaultRequirements(),
            imageUrl: "https://image.dev.null",
            input: Input({inputType: InputType.Url, data: bytes("https://input.dev.null")}),
            offer: defaultOffer()
        });
    }

    function request(uint32 idx, Offer memory offer) public view override returns (ProofRequest memory) {
        return ProofRequest({
            id: RequestIdLibrary.from(addr(), idx, true),
            requirements: defaultRequirements(),
            imageUrl: "https://image.dev.null",
            input: Input({inputType: InputType.Url, data: bytes("https://input.dev.null")}),
            offer: offer
        });
    }

    function sign(ProofRequest calldata) public view override returns (bytes memory) {
        return expectedSignature;
    }

    function signLockRequest(LockRequest calldata) public view override returns (bytes memory) {
        return expectedSignature;
    }

    function execute(address target, bytes memory data) public {
        vm.prank(signer.addr);
        smartWallet.execute(target, data, 0);
    }

    function execute(address target, bytes memory data, uint256 value) public {
        vm.prank(signer.addr);
        smartWallet.execute(target, data, value);
    }

    function setExpectedSignature(bytes memory _expectedSignature) public {
        smartWallet.setExpectedSignature(_expectedSignature);
    }
}
