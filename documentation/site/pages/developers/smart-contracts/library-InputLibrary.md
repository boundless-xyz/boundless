# InputLibrary

## State Variables

### INPUT_TYPE

```solidity
string constant INPUT_TYPE = "Input(uint8 inputType,bytes data)";
```

### INPUT_TYPEHASH

```solidity
bytes32 constant INPUT_TYPEHASH = keccak256(bytes(INPUT_TYPE));
```

## Functions

### createInlineInput

Creates an inline input.

```solidity
function createInlineInput(bytes memory inlineData) internal pure returns (Input memory);
```

**Parameters**

| Name         | Type    | Description                    |
| ------------ | ------- | ------------------------------ |
| `inlineData` | `bytes` | The data for the inline input. |

**Returns**

| Name     | Type    | Description                                             |
| -------- | ------- | ------------------------------------------------------- |
| `<none>` | `Input` | An Input struct with type Inline and the provided data. |

### createUrlInput

Creates a URL input.

```solidity
function createUrlInput(string memory url) internal pure returns (Input memory);
```

**Parameters**

| Name  | Type     | Description            |
| ----- | -------- | ---------------------- |
| `url` | `string` | The URL for the input. |

**Returns**

| Name     | Type    | Description                                                 |
| -------- | ------- | ----------------------------------------------------------- |
| `<none>` | `Input` | An Input struct with type Url and the provided URL as data. |

### eip712Digest

Computes the EIP-712 digest for the given input.

```solidity
function eip712Digest(Input memory input) internal pure returns (bytes32);
```

**Parameters**

| Name    | Type    | Description                          |
| ------- | ------- | ------------------------------------ |
| `input` | `Input` | The input to compute the digest for. |

**Returns**

| Name     | Type      | Description                      |
| -------- | --------- | -------------------------------- |
| `<none>` | `bytes32` | The EIP-712 digest of the input. |
