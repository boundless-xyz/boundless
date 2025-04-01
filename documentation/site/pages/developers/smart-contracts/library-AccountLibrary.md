# AccountLibrary

## Functions

### requestFlags

Gets the locked and fulfilled request flags for the request with the given index.

```solidity
function requestFlags(Account storage self, uint32 idx) internal view returns (bool locked, bool fulfilled);
```

**Parameters**

| Name   | Type      | Description                                |
| ------ | --------- | ------------------------------------------ |
| `self` | `Account` | The account to get the request flags from. |
| `idx`  | `uint32`  | The index of the request.                  |

**Returns**

| Name        | Type   | Description                                        |
| ----------- | ------ | -------------------------------------------------- |
| `locked`    | `bool` | True if the request is locked, false otherwise.    |
| `fulfilled` | `bool` | True if the request is fulfilled, false otherwise. |

### setRequestFlags

Sets the locked and fulfilled request flags for the request with the given index.

_The given value of flags will be applied with |= to the flags for the request. Least significant bit is locked, second-least significant is fulfilled._

```solidity
function setRequestFlags(Account storage self, uint32 idx, uint8 flags) internal;
```

**Parameters**

| Name    | Type      | Description                               |
| ------- | --------- | ----------------------------------------- |
| `self`  | `Account` | The account to set the request flags for. |
| `idx`   | `uint32`  | The index of the request.                 |
| `flags` | `uint8`   | The flags to set for the request.         |

### setRequestLocked

Sets the locked flag for the request with the given index.

_The flag indicates that a request has been locked now or in the past.
If a requests lock expires this flag will still be set._

```solidity
function setRequestLocked(Account storage self, uint32 idx) internal;
```

**Parameters**

| Name   | Type      | Description                              |
| ------ | --------- | ---------------------------------------- |
| `self` | `Account` | The account to set the request flag for. |
| `idx`  | `uint32`  | The index of the request.                |

### setRequestFulfilled

Sets the fulfilled flag for the request with the given index.

```solidity
function setRequestFulfilled(Account storage self, uint32 idx) internal;
```

**Parameters**

| Name   | Type      | Description                              |
| ------ | --------- | ---------------------------------------- |
| `self` | `Account` | The account to set the request flag for. |
| `idx`  | `uint32`  | The index of the request.                |
