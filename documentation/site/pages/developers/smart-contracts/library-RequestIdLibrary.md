# RequestIdLibrary

## State Variables

### SMART_CONTRACT_SIGNATURE_FLAG

```solidity
uint256 internal constant SMART_CONTRACT_SIGNATURE_FLAG = 1 << 192;
```

## Functions

### from

Creates a RequestId from a client address and a 32-bit index.

```solidity
function from(address client1, uint32 id) internal pure returns (RequestId);
```

**Parameters**

| Name      | Type      | Description                |
| --------- | --------- | -------------------------- |
| `client1` | `address` | The address of the client. |
| `id`      | `uint32`  | The 32-bit index.          |

**Returns**

| Name     | Type        | Description                |
| -------- | ----------- | -------------------------- |
| `<none>` | `RequestId` | The constructed RequestId. |

### from

Creates a RequestId from a client address, a 32-bit index, and a smart contract signature flag.

```solidity
function from(address client1, uint32 id, bool isSmartContractSig) internal pure returns (RequestId);
```

**Parameters**

| Name                 | Type      | Description                                          |
| -------------------- | --------- | ---------------------------------------------------- |
| `client1`            | `address` | The address of the client.                           |
| `id`                 | `uint32`  | The 32-bit index.                                    |
| `isSmartContractSig` | `bool`    | Whether the request uses a smart contract signature. |

**Returns**

| Name     | Type        | Description                |
| -------- | ----------- | -------------------------- |
| `<none>` | `RequestId` | The constructed RequestId. |

### clientAndIndex

Extracts the client address and index from a RequestId.

```solidity
function clientAndIndex(RequestId id) internal pure returns (address, uint32);
```

**Parameters**

| Name | Type        | Description                    |
| ---- | ----------- | ------------------------------ |
| `id` | `RequestId` | The RequestId to extract from. |

**Returns**

| Name     | Type      | Description                              |
| -------- | --------- | ---------------------------------------- |
| `<none>` | `address` | The client address and the 32-bit index. |
| `<none>` | `uint32`  |                                          |

### clientIndexAndSignatureType

Extracts the client address and index from a RequestId.

```solidity
function clientIndexAndSignatureType(RequestId id) internal pure returns (address, uint32, bool);
```

**Parameters**

| Name | Type        | Description                    |
| ---- | ----------- | ------------------------------ |
| `id` | `RequestId` | The RequestId to extract from. |

**Returns**

| Name     | Type      | Description                                                                                       |
| -------- | --------- | ------------------------------------------------------------------------------------------------- |
| `<none>` | `address` | The client address and the 32-bit index, and true if the signature is a smart contract signature. |
| `<none>` | `uint32`  |                                                                                                   |
| `<none>` | `bool`    |                                                                                                   |

### clientAndIsSmartContractSigned

```solidity
function clientAndIsSmartContractSigned(RequestId id) internal pure returns (address, bool);
```

### isSmartContractSigned

```solidity
function isSmartContractSigned(RequestId id) internal pure returns (bool);
```
