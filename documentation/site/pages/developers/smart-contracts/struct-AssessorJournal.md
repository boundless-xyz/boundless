# AssessorJournal

Represents the structured journal of the Assessor guest which verifies the signature(s)
from client(s) and that the requirements are met by claim digest(s) in the Merkle tree committed
to by the given root.

```solidity
struct AssessorJournal {
    AssessorCallback[] callbacks;
    Selector[] selectors;
    bytes32 root;
    address prover;
}
```
