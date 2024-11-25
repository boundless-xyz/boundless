// Copyright (c) 2024 RISC Zero, Inc.
//
// All rights reserved.

pragma solidity ^0.8.20;

import {Math} from "@openzeppelin/contracts/utils/math/Math.sol";
import {SafeCast} from "@openzeppelin/contracts/utils/math/SafeCast.sol";
import {IRiscZeroVerifier} from "risc0/IRiscZeroVerifier.sol";

import {
    IBoundlessMarket,
    Input,
    InputType,
    Offer,
    Predicate,
    PredicateType,
    ProofRequest,
    Requirements
} from "./IBoundlessMarket.sol";

library BoundlessMarketLib {
    using SafeCast for uint256;

    // EIP-712 SIGNATURE UTILITIES

    string constant EIP712_DOMAIN = "IBoundlessMarket";
    string constant EIP712_DOMAIN_VERSION = "1";

    // EIP-712 type strings of the structs from above.
    // NOTE: Complete type encoding for EIP-712 is a concatenation of the type string with the sorted list of its transitive dependencies.
    // See https://eips.ethereum.org/EIPS/eip-712#definition-of-encodetype
    string constant PREDICATE_TYPE = "Predicate(uint8 predicateType,bytes data)";
    bytes32 constant PREDICATE_TYPEHASH = keccak256(bytes(PREDICATE_TYPE));

    string constant INPUT_TYPE = "Input(uint8 inputType,bytes data)";
    bytes32 constant INPUT_TYPEHASH = keccak256(bytes(INPUT_TYPE));

    string constant REQUIREMENTS_TYPE = "Requirements(bytes32 imageId,Predicate predicate)";
    bytes32 constant REQUIREMENTS_TYPEHASH = keccak256(abi.encodePacked(REQUIREMENTS_TYPE, PREDICATE_TYPE));

    string constant OFFER_TYPE =
        "Offer(uint256 minPrice,uint256 maxPrice,uint64 biddingStart,uint32 rampUpPeriod,uint32 timeout,uint256 lockinStake)";
    bytes32 constant OFFER_TYPEHASH = keccak256(abi.encodePacked(OFFER_TYPE));

    string constant PROVINGREQUEST_TYPE =
        "ProofRequest(uint256 id,Requirements requirements,string imageUrl,Input input,Offer offer)";
    bytes32 constant PROVINGREQUEST_TYPEHASH =
        keccak256(abi.encodePacked(PROVINGREQUEST_TYPE, INPUT_TYPE, OFFER_TYPE, PREDICATE_TYPE, REQUIREMENTS_TYPE));

    function eip712Digest(ProofRequest memory request) internal pure returns (bytes32) {
        return keccak256(
            abi.encode(
                BoundlessMarketLib.PROVINGREQUEST_TYPEHASH,
                request.id,
                eip712Digest(request.requirements),
                keccak256(bytes(request.imageUrl)),
                eip712Digest(request.input),
                eip712Digest(request.offer)
            )
        );
    }

    function eip712Digest(Requirements memory requirements) internal pure returns (bytes32) {
        return keccak256(
            abi.encode(
                BoundlessMarketLib.REQUIREMENTS_TYPEHASH, requirements.imageId, eip712Digest(requirements.predicate)
            )
        );
    }

    function eip712Digest(Input memory input) internal pure returns (bytes32) {
        return keccak256(abi.encode(BoundlessMarketLib.INPUT_TYPEHASH, input.inputType, keccak256(input.data)));
    }

    function eip712Digest(Offer memory offer) internal pure returns (bytes32) {
        return keccak256(
            abi.encode(
                BoundlessMarketLib.OFFER_TYPEHASH,
                offer.minPrice,
                offer.maxPrice,
                offer.biddingStart,
                offer.rampUpPeriod,
                offer.timeout,
                offer.lockinStake
            )
        );
    }

    function eip712Digest(Predicate memory predicate) internal pure returns (bytes32) {
        return keccak256(
            abi.encode(BoundlessMarketLib.PREDICATE_TYPEHASH, predicate.predicateType, keccak256(predicate.data))
        );
    }

    // REQUEST ID UTILITIES

    function requestId(address client, uint32 id) internal pure returns (uint256) {
        return uint256(uint160(client)) << 32 | uint256(id);
    }

    function requestFrom(uint256 id) internal pure returns (address) {
        if (id & (uint256(type(uint64).max) << 192) != 0) {
            revert IBoundlessMarket.InvalidRequest();
        }
        return address(uint160(id >> 32));
    }

    function requestIndex(uint256 id) internal pure returns (uint32) {
        if (id & (uint256(type(uint64).max) << 192) != 0) {
            revert IBoundlessMarket.InvalidRequest();
        }
        return uint32(id);
    }

    // OFFER UTILITIES

    function requireValid(Offer memory offer) internal pure {
        if (offer.rampUpPeriod > offer.timeout) {
            revert IBoundlessMarket.InvalidRequest();
        }
        if (offer.minPrice > offer.maxPrice) {
            revert IBoundlessMarket.InvalidRequest();
        }
    }

    // Calculates the earliest block at which the offer will be worth at least the given price.
    function blockAtPrice(Offer memory offer, uint256 price) internal pure returns (uint64) {
        if (price > offer.maxPrice) {
            revert IBoundlessMarket.InvalidRequest();
        }

        if (price <= offer.minPrice) {
            return 0;
        }

        // Note: If we are in this branch, then
        //  offer.minPrice < offer.maxPrice
        // This means it is safe to divide by the difference

        uint256 rise = uint256(offer.maxPrice - offer.minPrice);
        uint256 run = uint256(offer.rampUpPeriod);

        uint256 delta = Math.ceilDiv(uint256(price - offer.minPrice) * run, rise);
        return offer.biddingStart + delta.toUint64();
    }

    // Calculates the price at the given block.
    function priceAtBlock(Offer memory offer, uint64 _block) internal pure returns (uint256) {
        if (_block <= offer.biddingStart) {
            return offer.minPrice;
        }

        if (_block <= offer.biddingStart + offer.rampUpPeriod) {
            // Note: if we are in this branch, then 0 < offer.rampUpPeriod
            // This means it is safe to divide by offer.rampUpPeriod

            uint256 rise = uint256(offer.maxPrice - offer.minPrice);
            uint256 run = uint256(offer.rampUpPeriod);
            uint256 delta = _block - uint256(offer.biddingStart);

            // Note: delta <= run
            // This means (delta * rise) / run <= rise
            // This means price <= offer.maxPrice

            uint256 price = uint256(offer.minPrice) + (delta * rise) / run;
            return price;
        }

        return offer.maxPrice;
    }

    function deadline(Offer memory offer) internal pure returns (uint64) {
        return offer.biddingStart + offer.timeout;
    }

    // PREDICATE UTILITIES

    function createDigestMatchPredicate(bytes32 hash) internal pure returns (Predicate memory) {
        return Predicate({predicateType: PredicateType.DigestMatch, data: abi.encode(hash)});
    }

    function createPrefixMatchPredicate(bytes memory prefix) internal pure returns (Predicate memory) {
        return Predicate({predicateType: PredicateType.PrefixMatch, data: prefix});
    }

    function eval(Predicate memory predicate, bytes memory journal, bytes32 journalDigest)
        internal
        pure
        returns (bool)
    {
        if (predicate.predicateType == PredicateType.DigestMatch) {
            return bytes32(predicate.data) == journalDigest;
        } else if (predicate.predicateType == PredicateType.PrefixMatch) {
            return startsWith(journal, predicate.data);
        } else {
            revert("Unreachable code");
        }
    }

    function startsWith(bytes memory journal, bytes memory prefix) internal pure returns (bool) {
        if (journal.length < prefix.length) {
            return false;
        }
        if (prefix.length == 0) {
            return true;
        }
        bytes memory slice = new bytes(prefix.length);
        assembly {
            let dest := add(slice, 0x20)
            let src := add(journal, 0x20)
            for { let i := 0 } lt(i, mload(prefix)) { i := add(i, 0x20) } { mstore(add(dest, i), mload(add(src, i))) }
        }
        return keccak256(slice) == keccak256(prefix);
    }

    function createInlineInput(bytes memory inlineData) internal pure returns (Input memory) {
        return Input({inputType: InputType.Inline, data: inlineData});
    }

    // PREDICATE UTILITIES

    function createUrlInput(string memory url) internal pure returns (Input memory) {
        return Input({inputType: InputType.Url, data: bytes(url)});
    }

    // UPDRAGES UTILS

    /// @notice ABI encode the constructor args for this contract.
    /// @dev This function exists to provide a type-safe way to ABI-encode constructor args, for
    /// use in the deployment process with OpenZeppelin Upgrades. Must be kept in sync with the
    /// signature of the BoundlessMarket constructor.
    function encodeConstructorArgs(IRiscZeroVerifier verifier, bytes32 assessorId)
        internal
        pure
        returns (bytes memory)
    {
        return abi.encode(verifier, assessorId);
    }
}
