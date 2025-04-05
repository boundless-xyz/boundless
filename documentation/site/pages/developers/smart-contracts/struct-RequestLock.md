# RequestLock

Stores information about requests that have been locked.

_RequestLock is an internal structure that is modified at various points in the proof lifecycle.
Fields can be valid or invalid depending where in the lifecycle we are. Integrators should not rely on RequestLock
for determining the status of a request. Instead, they should always use BoundlessMarket's public functions.
Packed to fit into 3 slots._

```solidity
struct RequestLock {
    address prover;
    uint64 lockDeadline;
    uint24 deadlineDelta;
    uint8 requestLockFlags;
    uint96 price;
    uint96 stake;
    bytes32 requestDigest;
}
```
