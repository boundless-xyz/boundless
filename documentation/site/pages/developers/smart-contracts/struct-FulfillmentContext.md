# FulfillmentContext

A struct for storing validated fulfillment information in transient storage

*This struct is designed to be packed into a single uint256 for efficient transient storage*


```solidity
struct FulfillmentContext {
    bool valid;
    uint96 price;
}
```

