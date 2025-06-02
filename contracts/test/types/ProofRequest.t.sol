// Copyright (c) 2025 RISC Zero, Inc.
//
// All rights reserved.

pragma solidity ^0.8.20;

import "forge-std/Test.sol";
import {Vm} from "forge-std/Vm.sol";
import {ProofRequest, ProofRequestLibrary} from "../../src/types/ProofRequest.sol";
import {Requirements} from "../../src/types/Requirements.sol";
import {Input, InputType} from "../../src/types/Input.sol";
import {Predicate, PredicateType, PredicateLibrary} from "../../src/types/Predicate.sol";
import {Callback} from "../../src/types/Callback.sol";
import {Offer} from "../../src/types/Offer.sol";
import {Account} from "../../src/types/Account.sol";
import {RequestId, RequestIdLibrary} from "../../src/types/RequestId.sol";
import {IBoundlessMarket} from "../../src/IBoundlessMarket.sol";

/// @dev Wrapper contract to test ProofRequest library functions. The library functions use
/// inputs of type calldata, so this contract enables our tests to make external calls that have calldata
/// to those functions.
contract ProofRequestTestContract {
    mapping(address => Account) accounts;

    function validate(ProofRequest calldata request) external pure returns (uint64, uint64) {
        return request.validate();
    }

    function setRequestFulfilled(address wallet1, uint32 idx1) external {
        accounts[wallet1].setRequestFulfilled(idx1);
    }

    function setRequestLocked(address wallet1, uint32 idx1) external {
        accounts[wallet1].setRequestLocked(idx1);
    }
}

contract MockERC1271Wallet {
    bytes4 internal constant MAGICVALUE = 0x1626ba7e; // bytes4(keccak256("isValidSignature(bytes32,bytes)")

    function isValidSignature(bytes32, bytes calldata) public pure returns (bytes4) {
        return MAGICVALUE;
    }
}

contract MockInvalidERC1271Wallet {
    function isValidSignature(bytes32, bytes calldata) public pure returns (bytes4) {
        return 0xdeadbeef;
    }
}

contract ProofRequestTest is Test {
    address wallet = address(0x123);
    uint32 idx = 1;
    bytes32 constant APP_IMAGE_ID = 0x0000000000000000000000000000000000000000000000000000000000000001;
    bytes32 constant SET_BUILDER_IMAGE_ID = 0x0000000000000000000000000000000000000000000000000000000000000002;
    bytes32 constant ASSESSOR_IMAGE_ID = 0x0000000000000000000000000000000000000000000000000000000000000003;
    Vm.Wallet clientWallet;
    Vm.Wallet proverWallet;

    ProofRequest defaultProofRequest;

    ProofRequestTestContract requestContract = new ProofRequestTestContract();

    function setUp() public {
        clientWallet = vm.createWallet("CLIENT");
        proverWallet = vm.createWallet("PROVER");

        defaultProofRequest = ProofRequest({
            id: RequestIdLibrary.from(wallet, idx),
            requirements: Requirements({
                imageId: APP_IMAGE_ID,
                predicate: Predicate({
                    predicateType: PredicateType.DigestMatch,
                    data: abi.encode(sha256(bytes("GUEST JOURNAL")))
                }),
                callback: Callback({gasLimit: 0, addr: address(0)}),
                selector: bytes4(0)
            }),
            imageUrl: "https://image.dev.null",
            input: Input({inputType: InputType.Url, data: bytes("https://input.dev.null")}),
            offer: Offer({
                minPrice: 1 ether,
                maxPrice: 2 ether,
                biddingStart: uint64(block.timestamp),
                rampUpPeriod: uint32(10),
                timeout: uint32(100),
                lockTimeout: uint32(100),
                lockStake: 1 ether
            })
        });
    }

    function testValidateBasic() public view {
        ProofRequest memory request = defaultProofRequest;
        Offer memory offer = request.offer;

        (uint64 lockDeadline, uint64 deadline) = requestContract.validate(request);
        assertEq(deadline, offer.deadline(), "Deadline should match the offer deadline");
        assertEq(lockDeadline, offer.lockDeadline(), "Lock deadline should match the offer lock deadline");
    }

    function testValidateInvalidPriceParameters() public {
        ProofRequest memory request = defaultProofRequest;
        request.offer.minPrice = 2 ether;
        request.offer.maxPrice = 1 ether;

        vm.expectRevert(IBoundlessMarket.InvalidRequest.selector);
        requestContract.validate(request);
    }

    function testValidateInvalidTimeoutParameters() public {
        ProofRequest memory request = defaultProofRequest;
        request.offer.lockTimeout = 10;
        request.offer.timeout = 5;

        vm.expectRevert(IBoundlessMarket.InvalidRequest.selector);
        requestContract.validate(request);

        request.offer.lockTimeout = 5;
        request.offer.timeout = 10;
        request.offer.rampUpPeriod = 8;
        vm.expectRevert(IBoundlessMarket.InvalidRequest.selector);
        requestContract.validate(request);

        // sanity check
        request.offer.timeout = 10;
        request.offer.lockTimeout = 5;
        request.offer.rampUpPeriod = 5;
        requestContract.validate(request);
    }

    function testValidateInvalidLockTimeoutLength() public {
        ProofRequest memory request = defaultProofRequest;
        // Difference exceeds what can be stored in the RequestLock type.
        request.offer.lockTimeout = 5;
        request.offer.timeout = type(uint32).max;

        vm.expectRevert(IBoundlessMarket.InvalidRequest.selector);
        requestContract.validate(request);
    }
}
