# FulfillmentContextLibrary

Library for packing, unpacking, and storing FulfillmentContext structs

_Uses bit manipulation to pack all fields into a single uint256 for transient storage_

## State Variables

### VALID_MASK

```solidity
uint256 private constant VALID_MASK = 1 << 127;
```

### PRICE_MASK

```solidity
uint256 private constant PRICE_MASK = (1 << 96) - 1;
```

## Functions

### pack

Packs the struct into a single 256-bit slots and sets the validation bit.

```solidity
function pack(FulfillmentContext memory x) internal pure returns (uint256);
```

**Parameters**

| Name | Type                 | Description                           |
| ---- | -------------------- | ------------------------------------- |
| `x`  | `FulfillmentContext` | The FulfillmentContext struct to pack |

**Returns**

| Name     | Type      | Description                                   |
| -------- | --------- | --------------------------------------------- |
| `<none>` | `uint256` | Packed uint256 containing valid bit and price |

### unpack

Unpacks the struct from a single uint256

```solidity
function unpack(uint256 packed) internal pure returns (FulfillmentContext memory);
```

**Parameters**

| Name     | Type      | Description                                       |
| -------- | --------- | ------------------------------------------------- |
| `packed` | `uint256` | Packed uint256 containing the valid bit and price |

**Returns**

| Name     | Type                 | Description                            |
| -------- | -------------------- | -------------------------------------- |
| `<none>` | `FulfillmentContext` | The unpacked FulfillmentContext struct |

### store

Packs and stores the object to transient storage

```solidity
function store(FulfillmentContext memory x, bytes32 requestDigest) internal;
```

**Parameters**

| Name            | Type                 | Description                               |
| --------------- | -------------------- | ----------------------------------------- |
| `x`             | `FulfillmentContext` | The FulfillmentContext struct to store    |
| `requestDigest` | `bytes32`            | The storage key for the transient storage |

### load

Loads from transient storage and unpacks to FulfillmentContext

```solidity
function load(bytes32 requestDigest) internal view returns (FulfillmentContext memory);
```

**Parameters**

| Name            | Type      | Description                  |
| --------------- | --------- | ---------------------------- |
| `requestDigest` | `bytes32` | The storage key to load from |

**Returns**

| Name     | Type                 | Description                                       |
| -------- | -------------------- | ------------------------------------------------- |
| `<none>` | `FulfillmentContext` | The loaded and unpacked FulfillmentContext struct |
