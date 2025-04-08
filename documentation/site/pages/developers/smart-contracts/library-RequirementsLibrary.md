# RequirementsLibrary

## State Variables

### REQUIREMENTS_TYPE

```solidity
string constant REQUIREMENTS_TYPE =
    "Requirements(bytes32 imageId,Callback callback,Predicate predicate,bytes4 selector)";
```

### REQUIREMENTS_TYPEHASH

```solidity
bytes32 constant REQUIREMENTS_TYPEHASH =
    keccak256(abi.encodePacked(REQUIREMENTS_TYPE, CallbackLibrary.CALLBACK_TYPE, PredicateLibrary.PREDICATE_TYPE));
```

## Functions

### eip712Digest

```solidity
function eip712Digest(Requirements memory requirements) internal pure returns (bytes32);
```
