# Fulfillment

Represents the information posted by the prover to fulfill a request and get paid.

```solidity
struct Fulfillment {
    RequestId id;
    bytes32 requestDigest;
    bytes32 imageId;
    bytes journal;
    bytes seal;
}
```
