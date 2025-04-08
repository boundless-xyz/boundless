# FulfillmentContext

A struct for storing validated fulfillment information in transient storage

_This struct is designed to be packed into a single uint256 for efficient transient storage_

```solidity
struct FulfillmentContext {
    bool valid;
    uint96 price;
}
```
