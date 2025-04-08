# MerkleProofish

## Functions

### processTree

```solidity
function processTree(bytes32[] memory leaves) internal pure returns (bytes32 root);
```

### _hashPair

_Sorts the pair (a, b) and hashes the result._

```solidity
function _hashPair(bytes32 a, bytes32 b) internal pure returns (bytes32);
```

### _efficientHash

_Implementation of keccak256(abi.encode(a, b)) that doesn't allocate or expand memory._

```solidity
function _efficientHash(bytes32 a, bytes32 b) private pure returns (bytes32 value);
```
