// Copyright (c) 2025 RISC Zero, Inc.
//
// All rights reserved.
pragma solidity ^0.8.24;

import {Test} from "forge-std/Test.sol";
import {SmartContractClient} from "../src/SmartContractClient.sol";
import {ProofRequest} from "boundless-market/types/ProofRequest.sol";
import {PredicateType} from "boundless-market/types/Predicate.sol";
import {ImageID} from "boundless-market/libraries/UtilImageID.sol";
import {RequestId, RequestIdLibrary} from "boundless-market/types/RequestId.sol";
import {IBoundlessMarket} from "boundless-market/IBoundlessMarket.sol";

contract MockBoundlessMarket {
    bytes32 public immutable DOMAIN_SEPARATOR;

    constructor() {
        DOMAIN_SEPARATOR = keccak256("boundless-market");
    }

    function eip712DomainSeparator() external view returns (bytes32) {
        return DOMAIN_SEPARATOR;
    }
}

contract TestContract {
    uint256 public value;

    function setValue(uint256 _value) external {
        value = _value;
    }
}

contract SmartContractClientTest is Test {
    SmartContractClient public client;
    MockBoundlessMarket public market;
    address public owner;
    address public user;
    uint32 public constant START_DAY = 100;
    uint32 public constant END_DAY = 200;

    function setUp() public {
        owner = address(0x1);
        user = address(0x2);
        market = new MockBoundlessMarket();
        client = new SmartContractClient(owner, address(market), START_DAY, END_DAY);
    }

    function test_Receive() public {
        vm.deal(user, 1 ether);
        vm.prank(user);
        (bool success,) = address(client).call{value: 1 ether}("");
        assertTrue(success);
        assertEq(address(client).balance, 1 ether);
    }

    function test_IsValidSignature() public view {
        // Create a valid proof request
        ProofRequest memory request = _createValidProofRequest(START_DAY + 1);
        bytes memory signature = abi.encode(request);
        bytes32 requestHash = _hashTypedData(request.eip712Digest());

        // Test valid signature
        bytes4 result = client.isValidSignature(requestHash, signature);
        assertEq(bytes32(result), bytes32(bytes4(0x1626ba7e))); // MAGICVALUE
    }

    function test_IsValidSignatureInvalidDay() public view {
        // Create a proof request with invalid day
        ProofRequest memory request = _createValidProofRequest(START_DAY - 1);
        bytes memory signature = abi.encode(request);
        bytes32 requestHash = _hashTypedData(request.eip712Digest());

        // Test invalid day
        bytes4 result = client.isValidSignature(requestHash, signature);
        assertEq(bytes32(result), bytes32(bytes4(0xffffffff)));
    }

    function test_IsValidSignatureInvalidImageId() public view {
        // Create a proof request with invalid image ID
        ProofRequest memory request = _createValidProofRequest(START_DAY + 1);
        request.requirements.imageId = bytes32(0);
        bytes memory signature = abi.encode(request);
        bytes32 requestHash = _hashTypedData(request.eip712Digest());

        // Test invalid image ID
        bytes4 result = client.isValidSignature(requestHash, signature);
        assertEq(bytes32(result), bytes32(bytes4(0xffffffff)));
    }

    function _createValidProofRequest(uint32 daysSinceEpoch) internal pure returns (ProofRequest memory) {
        ProofRequest memory request;
        request.requirements.imageId = ImageID.ECHO_ID;
        request.requirements.predicate.predicateType = PredicateType.DigestMatch;
        request.requirements.predicate.data = abi.encodePacked(sha256(abi.encodePacked(daysSinceEpoch)));
        request.id = RequestIdLibrary.from(address(0), daysSinceEpoch, true);
        return request;
    }

    function _hashTypedData(bytes32 dataHash) internal view returns (bytes32) {
        return keccak256(abi.encodePacked("\x19\x01", market.DOMAIN_SEPARATOR(), dataHash));
    }
}

