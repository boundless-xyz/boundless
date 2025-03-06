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

    function validateForLockRequest(ProofRequest calldata proofRequest, address wallet1, uint32 idx1)
        external
        view
        returns (uint64, uint64)
    {
        return proofRequest.validateForLockRequest(accounts, wallet1, idx1);
    }

    function validateForPriceRequest(ProofRequest calldata proofRequest) external view returns (uint64, uint64) {
        return proofRequest.validateForPriceRequest();
    }

    function verifyClientSignature(
        ProofRequest calldata proofRequest,
        bytes32 structHash,
        address addr,
        bytes calldata signature,
        bool isSmartContractSig
    ) external view {
        proofRequest.verifyClientSignature(structHash, addr, signature, isSmartContractSig);
    }

    function verifyProverSignature(
        ProofRequest calldata proofRequest,
        bytes32 structHash,
        address addr,
        bytes calldata signature
    ) external view {
        proofRequest.verifyProverSignature(structHash, addr, signature);
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
                }),
                callback: Callback({gasLimit: 0, addr: address(0)}),
                selector: bytes4(0)
            }),
            imageUrl: "https://image.dev.null",
            input: Input({inputType: InputType.Url, data: bytes("https://input.dev.null")}),
            offer: Offer({
                minPrice: 1 ether,
                maxPrice: 2 ether,
                biddingStart: uint64(block.number),
                rampUpPeriod: uint32(10),
                timeout: uint32(100),
                lockTimeout: uint32(100),
                lockStake: 1 ether
            })
        });
    }

    function testValidateForLockRequest() public view {
        ProofRequest memory proofRequest = defaultProofRequest;
        Offer memory offer = proofRequest.offer;

        (uint64 lockDeadline, uint64 deadline) = proofRequestContract.validateForLockRequest(proofRequest, wallet, idx);
        assertEq(deadline, offer.deadline(), "Deadline should match the offer deadline");
        assertEq(lockDeadline, offer.lockDeadline(), "Lock deadline should match the offer lock deadline");
    }

    function testValidateForLockRequestInvalidOffer() public {
        ProofRequest memory proofRequest = defaultProofRequest;
        proofRequest.offer.minPrice = 2 ether;
        proofRequest.offer.maxPrice = 1 ether;

        vm.expectRevert(IBoundlessMarket.InvalidRequest.selector);
        proofRequestContract.validateForLockRequest(proofRequest, wallet, idx);
    }

    function testValidateForLockRequestFulfilled() public {
        ProofRequest memory proofRequest = defaultProofRequest;
        proofRequestContract.setRequestFulfilled(wallet, 1);

        vm.expectRevert(abi.encodeWithSelector(IBoundlessMarket.RequestIsFulfilled.selector, proofRequest.id));
        proofRequestContract.validateForLockRequest(proofRequest, wallet, idx);
    }

    function testValidateForLockRequestLocked() public {
        proofRequestContract.setRequestLocked(wallet, 1);
        ProofRequest memory proofRequest = defaultProofRequest;

        vm.expectRevert(abi.encodeWithSelector(IBoundlessMarket.RequestIsLocked.selector, proofRequest.id));
        proofRequestContract.validateForLockRequest(proofRequest, wallet, idx);
    }

    function testValidateForPriceRequest() public view {
        ProofRequest memory proofRequest = defaultProofRequest;
        Offer memory offer = proofRequest.offer;

        (uint64 lockDeadline, uint64 deadline) = proofRequestContract.validateForPriceRequest(proofRequest);
        assertEq(deadline, offer.deadline(), "Deadline should match the offer deadline");
        assertEq(lockDeadline, offer.lockDeadline(), "Lock deadline should match the offer lock deadline");
    }

    function testValidateForPriceRequestFulfilled() public {
        ProofRequest memory proofRequest = defaultProofRequest;
        proofRequestContract.setRequestFulfilled(wallet, 1);
        Offer memory offer = proofRequest.offer;

        (uint64 lockDeadline, uint64 deadline) = proofRequestContract.validateForPriceRequest(proofRequest);
        assertEq(deadline, offer.deadline(), "Deadline should match the offer deadline");
        assertEq(lockDeadline, offer.lockDeadline(), "Lock deadline should match the offer lock deadline");
    }

    function testValidateForPriceRequestLocked() public {
        proofRequestContract.setRequestLocked(wallet, 1);
        ProofRequest memory proofRequest = defaultProofRequest;
        Offer memory offer = proofRequest.offer;

        (uint64 lockDeadline, uint64 deadline) = proofRequestContract.validateForPriceRequest(proofRequest);
        assertEq(deadline, offer.deadline(), "Deadline should match the offer deadline");
        assertEq(lockDeadline, offer.lockDeadline(), "Lock deadline should match the offer lock deadline");
    }

    function testValidateForPriceRequestInvalidOffer() public {
        ProofRequest memory proofRequest = defaultProofRequest;
        proofRequest.offer.minPrice = 2 ether;
        proofRequest.offer.maxPrice = 1 ether;

        vm.expectRevert(IBoundlessMarket.InvalidRequest.selector);
        proofRequestContract.validateForPriceRequest(proofRequest);
    }

    function testVerifyClientSignature() public {
        ProofRequest memory proofRequest = defaultProofRequest;
        proofRequest.id = RequestIdLibrary.from(clientWallet.addr, 1);
        bytes32 structHash = proofRequest.eip712Digest();
        (uint8 v, bytes32 r, bytes32 s) = vm.sign(clientWallet, structHash);
        bytes memory signature = abi.encodePacked(r, s, v);

        proofRequestContract.verifyClientSignature(proofRequest, structHash, clientWallet.addr, signature, false);
    }

    function testInvalidClientSignature() public {
        ProofRequest memory proofRequest = defaultProofRequest;
        proofRequest.id = RequestIdLibrary.from(clientWallet.addr, 1);
        bytes32 structHash = proofRequest.eip712Digest();
        (uint8 v, bytes32 r, bytes32 s) = vm.sign(proverWallet, structHash); // Signed by prover instead of client
        bytes memory signature = abi.encodePacked(r, s, v);

        vm.expectRevert(IBoundlessMarket.InvalidSignature.selector);
        proofRequestContract.verifyClientSignature(proofRequest, structHash, clientWallet.addr, signature, false);
    }

    function testSmartContractClientSignature() public {
        // Deploy a mock ERC1271 wallet that always returns isValidSignature.selector
        MockERC1271Wallet mockWallet = new MockERC1271Wallet();

        ProofRequest memory proofRequest = defaultProofRequest;
        proofRequest.id = RequestIdLibrary.from(address(mockWallet), 1, true);
        bytes32 structHash = proofRequest.eip712Digest();

        // Test with empty signature (should use request as signature)
        proofRequestContract.verifyClientSignature(proofRequest, structHash, address(mockWallet), "", true);

        // Test with non-empty signature
        bytes memory signature = "mock_signature";
        proofRequestContract.verifyClientSignature(proofRequest, structHash, address(mockWallet), signature, true);
    }

    function testInvalidSmartContractClientSignature() public {
        // Deploy a mock ERC1271 wallet that always returns invalid signature
        MockInvalidERC1271Wallet mockWallet = new MockInvalidERC1271Wallet();

        ProofRequest memory proofRequest = defaultProofRequest;
        proofRequest.id = RequestIdLibrary.from(address(mockWallet), 1, true);
        bytes32 structHash = proofRequest.eip712Digest();

        // Test with empty signature
        vm.expectRevert(IBoundlessMarket.InvalidSignature.selector);
        proofRequestContract.verifyClientSignature(proofRequest, structHash, address(mockWallet), "", true);

        // Test with non-empty signature
        bytes memory signature = "mock_signature";
        vm.expectRevert(IBoundlessMarket.InvalidSignature.selector);
        proofRequestContract.verifyClientSignature(proofRequest, structHash, address(mockWallet), signature, true);
    }

    function testVerifyProverSignature() public {
        ProofRequest memory proofRequest = defaultProofRequest;
        bytes32 structHash = proofRequest.eip712Digest();
        (uint8 v, bytes32 r, bytes32 s) = vm.sign(proverWallet, structHash);
        bytes memory signature = abi.encodePacked(r, s, v);

        proofRequestContract.verifyProverSignature(proofRequest, structHash, proverWallet.addr, signature);
    }

    function testInvalidProverSignature() public {
        ProofRequest memory proofRequest = defaultProofRequest;
        bytes32 structHash = proofRequest.eip712Digest();
        (uint8 v, bytes32 r, bytes32 s) = vm.sign(clientWallet, structHash); // Signed by client instead of prover
        bytes memory signature = abi.encodePacked(r, s, v);

        vm.expectRevert(IBoundlessMarket.InvalidSignature.selector);
        proofRequestContract.verifyProverSignature(proofRequest, structHash, proverWallet.addr, signature);
    }

    function testSmartContractProverSignature() public {
        // Deploy a mock ERC1271 wallet that always returns isValidSignature.selector
        MockERC1271Wallet mockWallet = new MockERC1271Wallet();

        ProofRequest memory proofRequest = defaultProofRequest;
        bytes32 structHash = proofRequest.eip712Digest();

        // Test with empty signature (should use request as signature)
        proofRequestContract.verifyProverSignature(proofRequest, structHash, address(mockWallet), "");

        // Test with non-empty signature
        bytes memory signature = "mock_signature";
        proofRequestContract.verifyProverSignature(proofRequest, structHash, address(mockWallet), signature);
    }

    function testInvalidSmartContractProverSignature() public {
        // Deploy a mock ERC1271 wallet that always returns invalid signature
        MockInvalidERC1271Wallet mockWallet = new MockInvalidERC1271Wallet();

        ProofRequest memory proofRequest = defaultProofRequest;
        bytes32 structHash = proofRequest.eip712Digest();

        // Test with empty signature
        vm.expectRevert(IBoundlessMarket.InvalidSignature.selector);
        proofRequestContract.verifyProverSignature(proofRequest, structHash, address(mockWallet), "");

        // Test with non-empty signature
        bytes memory signature = "mock_signature";
        vm.expectRevert(IBoundlessMarket.InvalidSignature.selector);
        proofRequestContract.verifyProverSignature(proofRequest, structHash, address(mockWallet), signature);
    }
}
