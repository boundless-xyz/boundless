# MerkleProofish


## Functions
### processTree


```solidity
function processTree(bytes32[] memory leaves) internal pure returns (bytes32 root);
```

### _hashPair

*Sorts the pair (a, b) and hashes the result.*


```solidity
function _hashPair(bytes32 a, bytes32 b) internal pure returns (bytes32);
```

### _efficientHash

*Implementation of keccak256(abi.encode(a, b)) that doesn't allocate or expand memory.*


```solidity
function _efficientHash(bytes32 a, bytes32 b) private pure returns (bytes32 value);
```

