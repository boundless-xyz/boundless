# PredicateLibrary

## State Variables

### PREDICATE_TYPE

```solidity
string constant PREDICATE_TYPE = "Predicate(uint8 predicateType,bytes data)";
```

### PREDICATE_TYPEHASH

```solidity
bytes32 constant PREDICATE_TYPEHASH = keccak256(bytes(PREDICATE_TYPE));
```

## Functions

### createDigestMatchPredicate

Creates a digest match predicate.

```solidity
function createDigestMatchPredicate(bytes32 hash) internal pure returns (Predicate memory);
```

**Parameters**

| Name   | Type      | Description        |
| ------ | --------- | ------------------ |
| `hash` | `bytes32` | The hash to match. |

**Returns**

| Name     | Type        | Description                                                     |
| -------- | ----------- | --------------------------------------------------------------- |
| `<none>` | `Predicate` | A Predicate struct with type DigestMatch and the provided hash. |

### createPrefixMatchPredicate

Creates a prefix match predicate.

```solidity
function createPrefixMatchPredicate(bytes memory prefix) internal pure returns (Predicate memory);
```

**Parameters**

| Name     | Type    | Description          |
| -------- | ------- | -------------------- |
| `prefix` | `bytes` | The prefix to match. |

**Returns**

| Name     | Type        | Description                                                       |
| -------- | ----------- | ----------------------------------------------------------------- |
| `<none>` | `Predicate` | A Predicate struct with type PrefixMatch and the provided prefix. |

### eval

Evaluates the predicate against the given journal and journal digest.

```solidity
function eval(Predicate memory predicate, bytes memory journal, bytes32 journalDigest) internal pure returns (bool);
```

**Parameters**

| Name            | Type        | Description                      |
| --------------- | ----------- | -------------------------------- |
| `predicate`     | `Predicate` | The predicate to evaluate.       |
| `journal`       | `bytes`     | The journal to evaluate against. |
| `journalDigest` | `bytes32`   | The digest of the journal.       |

**Returns**

| Name     | Type   | Description                                          |
| -------- | ------ | ---------------------------------------------------- |
| `<none>` | `bool` | True if the predicate is satisfied, false otherwise. |

### startsWith

Checks if the journal starts with the given prefix.

```solidity
function startsWith(bytes memory journal, bytes memory prefix) internal pure returns (bool);
```

**Parameters**

| Name      | Type    | Description              |
| --------- | ------- | ------------------------ |
| `journal` | `bytes` | The journal to check.    |
| `prefix`  | `bytes` | The prefix to check for. |

**Returns**

| Name     | Type   | Description                                                  |
| -------- | ------ | ------------------------------------------------------------ |
| `<none>` | `bool` | True if the journal starts with the prefix, false otherwise. |

### eip712Digest

Computes the EIP-712 digest for the given predicate.

```solidity
function eip712Digest(Predicate memory predicate) internal pure returns (bytes32);
```

**Parameters**

| Name        | Type        | Description                              |
| ----------- | ----------- | ---------------------------------------- |
| `predicate` | `Predicate` | The predicate to compute the digest for. |

**Returns**

| Name     | Type      | Description                          |
| -------- | --------- | ------------------------------------ |
| `<none>` | `bytes32` | The EIP-712 digest of the predicate. |
