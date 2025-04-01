# RequestLockLibrary

## State Variables

### PROVER_PAID_DURING_LOCK_FLAG

```solidity
uint8 internal constant PROVER_PAID_DURING_LOCK_FLAG = 1 << 0;
```

### PROVER_PAID_AFTER_LOCK_FLAG

```solidity
uint8 internal constant PROVER_PAID_AFTER_LOCK_FLAG = 1 << 1;
```

### SLASHED_FLAG

```solidity
uint8 internal constant SLASHED_FLAG = 1 << 2;
```

## Functions

### deadline

Calculates the deadline for the locked request.

```solidity
function deadline(RequestLock memory requestLock) internal pure returns (uint64);
```

**Parameters**

| Name          | Type          | Description                                     |
| ------------- | ------------- | ----------------------------------------------- |
| `requestLock` | `RequestLock` | The request lock to calculate the deadline for. |

**Returns**

| Name     | Type     | Description                   |
| -------- | -------- | ----------------------------- |
| `<none>` | `uint64` | The deadline for the request. |

### setProverPaidBeforeLockDeadline

```solidity
function setProverPaidBeforeLockDeadline(RequestLock storage requestLock) internal;
```

### setProverPaidAfterLockDeadline

```solidity
function setProverPaidAfterLockDeadline(RequestLock storage requestLock, address prover) internal;
```

### setSlashed

```solidity
function setSlashed(RequestLock storage requestLock) internal;
```

### isProverPaidBeforeLockDeadline

Returns true if the request was fulfilled by the locker
before the lock deadline and they have been paid.

```solidity
function isProverPaidBeforeLockDeadline(RequestLock memory requestLock) internal pure returns (bool);
```

**Parameters**

| Name          | Type          | Description                |
| ------------- | ------------- | -------------------------- |
| `requestLock` | `RequestLock` | The request lock to check. |

**Returns**

| Name     | Type   | Description                                                                                          |
| -------- | ------ | ---------------------------------------------------------------------------------------------------- |
| `<none>` | `bool` | True if the request was fulfilled before the lock deadline and the prover was paid, false otherwise. |

### isProverPaidAfterLockDeadline

Checks if the request was fulfilled by any prover after the lock deadline.

```solidity
function isProverPaidAfterLockDeadline(RequestLock memory requestLock) internal pure returns (bool);
```

**Parameters**

| Name          | Type          | Description                |
| ------------- | ------------- | -------------------------- |
| `requestLock` | `RequestLock` | The request lock to check. |

**Returns**

| Name     | Type   | Description                                                                                        |
| -------- | ------ | -------------------------------------------------------------------------------------------------- |
| `<none>` | `bool` | True if the request is fulfilled after the lock deadline and the prover was paid, false otherwise. |

### isProverPaid

Checks if the locked request was fulfilled and _a_ prover was paid. The prover paid
could be the prover that locked, or a prover that filled after the lock deadline.

```solidity
function isProverPaid(RequestLock memory requestLock) internal pure returns (bool);
```

**Parameters**

| Name          | Type          | Description                |
| ------------- | ------------- | -------------------------- |
| `requestLock` | `RequestLock` | The request lock to check. |

**Returns**

| Name     | Type   | Description                                                                |
| -------- | ------ | -------------------------------------------------------------------------- |
| `<none>` | `bool` | True if the request is fulfilled after the lock deadline, false otherwise. |

### isSlashed

Checks if the request was slashed.

_Whether a request resulted in a slash does not indicate whether the request was fulfilled
since it is possible for a request to be fulfilled after a request lock has expired._

```solidity
function isSlashed(RequestLock memory requestLock) internal pure returns (bool);
```

**Parameters**

| Name          | Type          | Description                |
| ------------- | ------------- | -------------------------- |
| `requestLock` | `RequestLock` | The request lock to check. |

**Returns**

| Name     | Type   | Description                                      |
| -------- | ------ | ------------------------------------------------ |
| `<none>` | `bool` | True if the request is slashed, false otherwise. |

### clearSlot2

```solidity
function clearSlot2(RequestLock storage requestLock) private;
```

### clearSlot1And2

```solidity
function clearSlot1And2(RequestLock storage requestLock) private;
```
