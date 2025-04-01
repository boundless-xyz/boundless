# AssessorCommitmentLibrary

## State Variables

### ASSESSOR_COMMITMENT_TYPE

_Id is uint256 as for user defined types, the eip712 type hash uses the underlying type._

```solidity
string constant ASSESSOR_COMMITMENT_TYPE =
    "AssessorCommitment(uint256 index,uint256 id,bytes32 requestDigest,bytes32 claimDigest)";
```

### ASSESSOR_COMMITMENT_TYPEHASH

```solidity
bytes32 constant ASSESSOR_COMMITMENT_TYPEHASH = keccak256(bytes(ASSESSOR_COMMITMENT_TYPE));
```

## Functions

### eip712Digest

Computes the EIP-712 digest for the given commitment.

```solidity
function eip712Digest(AssessorCommitment memory commitment) internal pure returns (bytes32);
```

**Parameters**

| Name         | Type                 | Description                               |
| ------------ | -------------------- | ----------------------------------------- |
| `commitment` | `AssessorCommitment` | The commitment to compute the digest for. |

**Returns**

| Name     | Type      | Description                           |
| -------- | --------- | ------------------------------------- |
| `<none>` | `bytes32` | The EIP-712 digest of the commitment. |
