# AssessorReceipt

Represents the output of the assessor and proof of correctness, allowing request fulfillment.

```solidity
struct AssessorReceipt {
    bytes seal;
    AssessorCallback[] callbacks;
    Selector[] selectors;
    address prover;
}
```
