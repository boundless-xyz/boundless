# AssessorCommitment

Represents the structured commitment used as a leaf in the Assessor guest Merkle tree guest.

```solidity
struct AssessorCommitment {
    uint256 index;
    RequestId id;
    bytes32 requestDigest;
    bytes32 claimDigest;
}
```
