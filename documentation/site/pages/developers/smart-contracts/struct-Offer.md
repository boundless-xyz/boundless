# Offer

Represents an offer and provides functions to validate and compute offer-related data.

```solidity
struct Offer {
    uint256 minPrice;
    uint256 maxPrice;
    uint64 biddingStart;
    uint32 rampUpPeriod;
    uint32 lockTimeout;
    uint32 timeout;
    uint256 lockStake;
}
```
