# CallbackLibrary

## State Variables

### CALLBACK_TYPE

```solidity
string constant CALLBACK_TYPE = "Callback(address addr,uint96 gasLimit)";
```

### CALLBACK_TYPEHASH

```solidity
bytes32 constant CALLBACK_TYPEHASH = keccak256(bytes(CALLBACK_TYPE));
```

## Functions

### eip712Digest

Computes the EIP-712 digest for the given callback

```solidity
function eip712Digest(Callback memory callback) internal pure returns (bytes32);
```

**Parameters**

| Name       | Type       | Description                            |
| ---------- | ---------- | -------------------------------------- |
| `callback` | `Callback` | The callback to compute the digest for |

**Returns**

| Name     | Type      | Description                        |
| -------- | --------- | ---------------------------------- |
| `<none>` | `bytes32` | The EIP-712 digest of the callback |
