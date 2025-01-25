// Copyright (c) 2025 RISC Zero, Inc.
//
// All rights reserved.
pragma solidity ^0.8.20;

import {Math} from "@openzeppelin/contracts/utils/math/Math.sol";
import {SafeCast} from "@openzeppelin/contracts/utils/math/SafeCast.sol";
import {IBoundlessMarket} from "../IBoundlessMarket.sol";
import {RequestId} from "./RequestId.sol";

using OfferLibrary for Offer global;

/// @title Offer Struct and Library
/// @notice Represents an offer and provides functions to validate and compute offer-related data.
struct Offer {
    /// @notice Price at the start of the bidding period, it is minimum price a prover will receive for job.
    uint256 minPrice;
    /// @notice Price at the end of the bidding period, this is the maximum price the client will pay.
    uint256 maxPrice;
    /// @notice Block number at which bidding starts.
    uint64 biddingStart;
    /// @notice Length of the "ramp-up period," measured in blocks since bidding start.
    /// @dev Once bidding starts, the price begins to "ramp-up." During this time, the price rises each block until it reaches maxPrice.
    uint32 rampUpPeriod;
    /// @notice Timeout for delivering the proof, expressed as a number of blocks from bidding start.
    /// @dev Once locked-in, if a valid proof is not submitted before this deadline, the prover can be "slashed," which refunds the price to the requester.
    uint32 timeout;
    /// @notice Bidders must stake this amount as part of their bid.
    uint256 lockStake;
}

library OfferLibrary {
    using SafeCast for uint256;

    string constant OFFER_TYPE =
        "Offer(uint256 minPrice,uint256 maxPrice,uint64 biddingStart,uint32 rampUpPeriod,uint32 timeout,uint256 lockStake)";
    bytes32 constant OFFER_TYPEHASH = keccak256(abi.encodePacked(OFFER_TYPE));

    /// @notice Validates that price, ramp-up, timeout, and deadline are internally consistent and the offer has not expired.
    /// @param offer The offer to validate.
    /// @param requestId The ID of the request associated with the offer.
    /// @return deadline1 The deadline for the offer.
    function validate(Offer memory offer, RequestId requestId) internal view returns (uint64 deadline1) {
        if (offer.rampUpPeriod > offer.timeout) {
            revert IBoundlessMarket.InvalidRequest();
        }
        if (offer.minPrice > offer.maxPrice) {
            revert IBoundlessMarket.InvalidRequest();
        }
        deadline1 = offer.deadline();
        if (deadline1 < block.number) {
            revert IBoundlessMarket.RequestIsExpired(requestId, deadline1);
        }
    }

    /// @notice Calculates the earliest block at which the offer will be worth at least the given price.
    /// @param offer The offer to calculate for.
    /// @param price The price to calculate the block for.
    /// @return The earliest block at which the offer will be worth at least the given price.
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

    /// @notice Calculates the price at the given block.
    /// @param offer The offer to calculate for.
    /// @param _block The block to calculate the price for.
    /// @return The price at the given block.
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

    /// @notice Calculates the deadline for the offer.
    /// @param offer The offer to calculate the deadline for.
    /// @return The deadline for the offer.
    function deadline(Offer memory offer) internal pure returns (uint64) {
        return offer.biddingStart + offer.timeout;
    }

    /// @notice Computes the EIP-712 digest for the given offer.
    /// @param offer The offer to compute the digest for.
    /// @return The EIP-712 digest of the offer.
    function eip712Digest(Offer memory offer) internal pure returns (bytes32) {
        return keccak256(
            abi.encode(
                OFFER_TYPEHASH,
                offer.minPrice,
                offer.maxPrice,
                offer.biddingStart,
                offer.rampUpPeriod,
                offer.timeout,
                offer.lockStake
            )
        );
    }
}
