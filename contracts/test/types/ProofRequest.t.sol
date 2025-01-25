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
import {Offer} from "../../src/types/Offer.sol";
import {Account} from "../../src/types/Account.sol";
import {RequestId, RequestIdLibrary} from "../../src/types/RequestId.sol";
import {IBoundlessMarket} from "../../src/IBoundlessMarket.sol";

/// @dev Wrapper contract to test ProofRequest library functions. The library functions use
/// inputs of type calldata, so this contract enables our tests to make external calls that have calldata
/// to those functions.
contract ProofRequestTestContract {
    mapping(address => Account) accounts;

    function validateRequest(ProofRequest calldata proofRequest, address wallet1, uint32 idx1)
        external
        view
        returns (uint64)
    {
        return proofRequest.validateRequest(accounts, wallet1, idx1);
    }

    function verifyClientSignature(
        ProofRequest calldata proofRequest,
        bytes32 structHash,
        address addr,
        bytes calldata signature
    ) external pure returns (bytes32) {
        return proofRequest.verifyClientSignature(structHash, addr, signature);
    }

    function extractProverSignature(
        ProofRequest calldata proofRequest,
        bytes32 structHash,
        bytes calldata proverSignature
    ) external pure returns (address) {
        return proofRequest.extractProverSignature(structHash, proverSignature);
    }

    function setRequestFulfilled(address wallet1, uint32 idx1) external {
        accounts[wallet1].setRequestFulfilled(idx1);
    }

    function setRequestLocked(address wallet1, uint32 idx1) external {
        accounts[wallet1].setRequestLocked(idx1);
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

    ProofRequestTestContract proofRequestContract = new ProofRequestTestContract();

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
                })
            }),
            imageUrl: "https://image.dev.null",
            input: Input({inputType: InputType.Url, data: bytes("https://input.dev.null")}),
            offer: Offer({
                minPrice: 1 ether,
                maxPrice: 2 ether,
                biddingStart: uint64(block.number),
                rampUpPeriod: uint32(10),
                timeout: uint32(100),
                lockStake: 1 ether
            })
        });
    }

    function testValidateRequest() public view {
        ProofRequest memory proofRequest = defaultProofRequest;
        Offer memory offer = proofRequest.offer;

        uint64 deadline = proofRequestContract.validateRequest(proofRequest, wallet, idx);
        assertEq(deadline, offer.deadline(), "Deadline should match the offer deadline");
    }

    function testValidateRequestInvalidOffer() public {
        ProofRequest memory proofRequest = defaultProofRequest;
        proofRequest.offer.minPrice = 2 ether;
        proofRequest.offer.maxPrice = 1 ether;

        vm.expectRevert(IBoundlessMarket.InvalidRequest.selector);
        proofRequestContract.validateRequest(proofRequest, wallet, idx);
    }

    function testValidateRequestFulfilled() public {
        ProofRequest memory proofRequest = defaultProofRequest;
        proofRequestContract.setRequestFulfilled(wallet, 1);

        vm.expectRevert(abi.encodeWithSelector(IBoundlessMarket.RequestIsFulfilled.selector, proofRequest.id));
        proofRequestContract.validateRequest(proofRequest, wallet, idx);
    }

    function testValidateRequestLocked() public {
        proofRequestContract.setRequestLocked(wallet, 1);
        ProofRequest memory proofRequest = defaultProofRequest;

        vm.expectRevert(abi.encodeWithSelector(IBoundlessMarket.RequestIsLocked.selector, proofRequest.id));
        proofRequestContract.validateRequest(proofRequest, wallet, idx);
    }

    function testVerifyClientSignature() public {
        ProofRequest memory proofRequest = defaultProofRequest;
        proofRequest.id = RequestIdLibrary.from(clientWallet.addr, 1);
        bytes32 structHash = proofRequest.eip712Digest();
        (uint8 v, bytes32 r, bytes32 s) = vm.sign(clientWallet, structHash);
        bytes memory signature = abi.encodePacked(r, s, v);

        bytes32 result =
            proofRequestContract.verifyClientSignature(proofRequest, structHash, clientWallet.addr, signature);
        assertEq(result, structHash, "Signature verification failed");
    }

    function testExtractProverSignature() public {
        ProofRequest memory proofRequest = defaultProofRequest;
        proofRequest.id = RequestIdLibrary.from(clientWallet.addr, 1);
        bytes32 structHash = proofRequest.eip712Digest();
        (uint8 vProver, bytes32 rProver, bytes32 sProver) = vm.sign(proverWallet, structHash);
        bytes memory proverSignature = abi.encodePacked(rProver, sProver, vProver);

        address prover = proofRequestContract.extractProverSignature(proofRequest, structHash, proverSignature);
        assertEq(prover, proverWallet.addr, "Prover address recovery failed");
    }

    function testInvalidClientSignature() public {
        ProofRequest memory proofRequest = defaultProofRequest;
        proofRequest.id = RequestIdLibrary.from(clientWallet.addr, 1);
        bytes32 structHash = proofRequest.eip712Digest();
        (uint8 v, bytes32 r, bytes32 s) = vm.sign(proverWallet, structHash); // Signed by prover instead of client
        bytes memory signature = abi.encodePacked(r, s, v);

        vm.expectRevert(IBoundlessMarket.InvalidSignature.selector);
        proofRequestContract.verifyClientSignature(proofRequest, structHash, clientWallet.addr, signature);
    }
}
