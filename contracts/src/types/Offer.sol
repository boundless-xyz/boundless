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
    /// @notice Time at which bidding starts, in seconds since the UNIX epoch.
    uint64 biddingStart;
    /// @notice Length of the "ramp-up period," measured in seconds since bidding start.
    /// @dev Once bidding starts, the price begins to "ramp-up." During this time, the price rises
    /// each block until it reaches `maxPrice.
    uint32 rampUpPeriod;
    /// @notice Timeout for the lock, expressed as seconds from bidding start.
    /// @dev Once locked, if a valid proof is not submitted before this deadline, the prover can
    /// be "slashed", which refunds the price to the requester and takes the prover stake.
    ///
    /// Additionally, the fee paid by the client is zero for proofs delivered after this time.
    /// Note that after this time, and before `timeout` a proof can still be delivered to fulfill
    /// the request. This applies both to locked and unlocked requests; if a proof is delivered
    /// after this timeout, no fee will be paid from the client.
    uint32 lockTimeout;
    /// @notice Timeout for the request, expressed as seconds from bidding start.
    /// @dev After this time the request is considered completely expired and can no longer be
    /// fulfilled. After this time, the `slash` action can be completed to finalize the transaction
    /// if it was locked but not fulfilled.
    uint32 timeout;
    /// @notice Bidders must stake this amount as part of their bid.
    uint256 lockStake;
}

library OfferLibrary {
    using SafeCast for uint256;

    string constant OFFER_TYPE =
        "Offer(uint256 minPrice,uint256 maxPrice,uint64 biddingStart,uint32 rampUpPeriod,uint32 lockTimeout,uint32 timeout,uint256 lockStake)";
    bytes32 constant OFFER_TYPEHASH = keccak256(abi.encodePacked(OFFER_TYPE));

    /// @notice Validates that price, ramp-up, timeout, and deadline are internally consistent and well formed.
    /// @param offer The offer to validate.
    /// @return lockDeadline1 The deadline for when a lock expires for the offer.
    /// @return deadline1 The deadline for the offer as a whole.
    function validate(Offer memory offer) internal pure returns (uint64 lockDeadline1, uint64 deadline1) {
        if (offer.minPrice > offer.maxPrice) {
            revert IBoundlessMarket.InvalidRequest();
        }
        if (offer.rampUpPeriod > offer.lockTimeout) {
            revert IBoundlessMarket.InvalidRequest();
        }
        if (offer.lockTimeout > offer.timeout) {
            revert IBoundlessMarket.InvalidRequest();
        }
        lockDeadline1 = offer.lockDeadline();
        deadline1 = offer.deadline();
        if (deadline1 - lockDeadline1 > type(uint24).max) {
            revert IBoundlessMarket.InvalidRequest();
        }
        // DO NOT MERGE: Make sure tests cover expired requests on each entrypoint, to ensure this change does not break security.
    }

    /// @notice Calculates the earliest time at which the offer will be worth at least the given price.
    /// @dev Returned time will always be in the range 0 to offer.biddingStart + offer.rampUpPeriod.
    /// @param offer The offer to calculate for.
    /// @param price The price to calculate the time for.
    /// @return The earliest time at which the offer will be worth at least the given price.
    function timeAtPrice(Offer memory offer, uint256 price) internal pure returns (uint64) {
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

    /// @notice Calculates the price at the given time.
    /// @dev Price increases linearly during the ramp-up period, then remains at the max price until
    /// the lock deadline. After the lock deadline, the price goes to zero. As a result, provers are
    /// paid no fee from the client for requests that are fulfilled after lock deadline. Note though
    /// that there may be a reward of stake available, if a prover failed to deliver on the request.
    /// @param offer The offer to calculate for.
    /// @param timestamp The time to calculate the price for, as a UNIX timestamp.
    /// @return The price at the given time.
    function priceAt(Offer memory offer, uint64 timestamp) internal pure returns (uint256) {
        if (timestamp <= offer.biddingStart) {
            return offer.minPrice;
        }

        if (timestamp > offer.lockDeadline()) {
            return 0;
        }

        if (timestamp <= offer.biddingStart + offer.rampUpPeriod) {
            // Note: if we are in this branch, then 0 < offer.rampUpPeriod
            // This means it is safe to divide by offer.rampUpPeriod

            uint256 rise = uint256(offer.maxPrice - offer.minPrice);
            uint256 run = uint256(offer.rampUpPeriod);
            uint256 delta = timestamp - uint256(offer.biddingStart);

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
    /// @return The deadline for the offer, as a UNIX timestamp.
    function deadline(Offer memory offer) internal pure returns (uint64) {
        return offer.biddingStart + offer.timeout;
    }

    /// @notice Calculates the lock deadline for the offer.
    /// @param offer The offer to calculate the lock deadline for.
    /// @return The lock deadline for the offer, as a UNIX timestamp.
    function lockDeadline(Offer memory offer) internal pure returns (uint64) {
        return offer.biddingStart + offer.lockTimeout;
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
                offer.lockTimeout,
                offer.timeout,
                offer.lockStake
            )
        );
    }
}
