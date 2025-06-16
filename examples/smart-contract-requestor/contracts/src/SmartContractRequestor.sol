// Copyright (c) 2025 RISC Zero, Inc.
//
// All rights reserved.
pragma solidity ^0.8.24;

import {IERC1271} from "@openzeppelin/contracts/interfaces/IERC1271.sol";
import {IERC20} from "@openzeppelin/contracts/token/ERC20/IERC20.sol";
import {ProofRequest} from "boundless-market/types/ProofRequest.sol";
import {ImageID} from "boundless-market/libraries/UtilImageID.sol";
import {RequestId} from "boundless-market/types/RequestId.sol";
import {PredicateType} from "boundless-market/types/Predicate.sol";
import {BoundlessMarketLib} from "boundless-market/libraries/BoundlessMarketLib.sol";
import {IBoundlessMarket} from "boundless-market/IBoundlessMarket.sol";

/// @dev Sample implementation of an ERC-1271 compliant Smart Contract Requestor for the Boundless Market.
contract SmartContractRequestor is IERC1271 {
    address private owner;

    // The magic value for ERC-1271 isValidSignature.
    bytes4 internal constant MAGICVALUE = 0x1626ba7e;
    // The image ID for the echo guest program.
    bytes32 private constant ECHO_ID = ImageID.ECHO_ID;
    // Cache of the BoundlessMarket domain separator.
    bytes32 private immutable BOUNDLESS_MARKET_DOMAIN;

    // The start day since epoch that we will start accepting requests.
    uint32 private immutable START_DAY_SINCE_EPOCH;
    // The end day since epoch that we will stop accepting requests.
    uint32 private immutable END_DAY_SINCE_EPOCH;

    constructor(address _owner, address _boundlessMarket, uint32 _startDaySinceEpoch, uint32 _endDaySinceEpoch) {
        owner = _owner;
        BOUNDLESS_MARKET_DOMAIN = IBoundlessMarket(_boundlessMarket).eip712DomainSeparator();
        START_DAY_SINCE_EPOCH = _startDaySinceEpoch;
        END_DAY_SINCE_EPOCH = _endDaySinceEpoch;
    }

    function isValidSignature(bytes32 requestHash, bytes memory signature) external view returns (bytes4) {
        // This smart contract client expects the full abi encoded ProofRequest to be provided as the signature.
        ProofRequest memory request = abi.decode(signature, (ProofRequest));

        // The request id acts as our nonce. Here we use the index of the request id to represent each day.
        // Boundless Market ensures each request id can only be fulfilled once, so the nonce property ensures
        // we will only ever pay for one request per day.
        (, uint32 daysSinceEpoch) = request.id.clientAndIndex();
        if (daysSinceEpoch < START_DAY_SINCE_EPOCH || daysSinceEpoch > END_DAY_SINCE_EPOCH) {
            return 0xffffffff;
        }

        // Validate that the request provided is as expected.
        // For this example, we check the image id is as expected, and that the predicate restricts
        // the output to match the day specified in the id.
        if (request.requirements.imageId != ECHO_ID) {
            return 0xffffffff;
        }

        // Validate the predicate type and data are correct. This ensures that the request was executed with
        // the correct input and resulted in the correct output. In this case it ensures that the input
        // to the request was the correct day since epoch that corresponds to the request id.
        if (request.requirements.predicate.predicateType != PredicateType.DigestMatch) {
            return 0xffffffff;
        }
        bytes32 expectedPredicate = sha256(abi.encodePacked(daysSinceEpoch));
        if (bytes32(request.requirements.predicate.data) != expectedPredicate) {
            return 0xffffffff;
        }

        // Validate that the EIP-712 hash of the request provided in the signature matches the hash that was
        // provided by BoundlessMarket. This ensures that Boundless is processing the same request that we have
        // validated.
        if (_hashTypedData(request.eip712Digest()) == requestHash) {
            return MAGICVALUE;
        }

        return 0xffffffff;
    }

    // Allow the wallet to receive ETH
    receive() external payable {}

    function execute(address target, bytes memory data, uint256 value) external payable {
        require(msg.sender == owner, "Not authorized");
        (bool success,) = target.call{value: value}(data);
        require(success, "Call failed");
    }

    /// @notice Creates an EIP-712 typed data hash
    function _hashTypedData(bytes32 dataHash) internal view returns (bytes32) {
        return keccak256(abi.encodePacked("\x19\x01", BOUNDLESS_MARKET_DOMAIN, dataHash));
    }
}
