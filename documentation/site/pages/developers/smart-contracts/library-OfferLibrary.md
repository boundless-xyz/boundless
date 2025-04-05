# OfferLibrary

## State Variables

### OFFER_TYPE

```solidity
string constant OFFER_TYPE =
    "Offer(uint256 minPrice,uint256 maxPrice,uint64 biddingStart,uint32 rampUpPeriod,uint32 lockTimeout,uint32 timeout,uint256 lockStake)";
```

### OFFER_TYPEHASH

```solidity
bytes32 constant OFFER_TYPEHASH = keccak256(abi.encodePacked(OFFER_TYPE));
```

## Functions

### validate

Validates that price, ramp-up, timeout, and deadline are internally consistent and the offer has not expired.

```solidity
function validate(Offer memory offer, RequestId requestId)
    internal
    view
    returns (uint64 lockDeadline1, uint64 deadline1);
```

**Parameters**

| Name        | Type        | Description                                      |
| ----------- | ----------- | ------------------------------------------------ |
| `offer`     | `Offer`     | The offer to validate.                           |
| `requestId` | `RequestId` | The ID of the request associated with the offer. |

**Returns**

| Name            | Type     | Description                                         |
| --------------- | -------- | --------------------------------------------------- |
| `lockDeadline1` | `uint64` | The deadline for when a lock expires for the offer. |
| `deadline1`     | `uint64` | The deadline for the offer as a whole.              |

### timeAtPrice

Calculates the earliest time at which the offer will be worth at least the given price.

_Returned time will always be in the range 0 to offer.biddingStart + offer.rampUpPeriod._

```solidity
function timeAtPrice(Offer memory offer, uint256 price) internal pure returns (uint64);
```

**Parameters**

| Name    | Type      | Description                          |
| ------- | --------- | ------------------------------------ |
| `offer` | `Offer`   | The offer to calculate for.          |
| `price` | `uint256` | The price to calculate the time for. |

**Returns**

| Name     | Type     | Description                                                                  |
| -------- | -------- | ---------------------------------------------------------------------------- |
| `<none>` | `uint64` | The earliest time at which the offer will be worth at least the given price. |

### priceAt

Calculates the price at the given time.

_Price increases linearly during the ramp-up period, then remains at the max price until
the lock deadline. After the lock deadline, the price goes to zero. As a result, provers are
paid no fee from the client for requests that are fulfilled after lock deadline. Note though
that there may be a reward of stake available, if a prover failed to deliver on the request._

```solidity
function priceAt(Offer memory offer, uint64 timestamp) internal pure returns (uint256);
```

**Parameters**

| Name        | Type     | Description                                               |
| ----------- | -------- | --------------------------------------------------------- |
| `offer`     | `Offer`  | The offer to calculate for.                               |
| `timestamp` | `uint64` | The time to calculate the price for, as a UNIX timestamp. |

**Returns**

| Name     | Type      | Description                  |
| -------- | --------- | ---------------------------- |
| `<none>` | `uint256` | The price at the given time. |

### deadline

Calculates the deadline for the offer.

```solidity
function deadline(Offer memory offer) internal pure returns (uint64);
```

**Parameters**

| Name    | Type    | Description                              |
| ------- | ------- | ---------------------------------------- |
| `offer` | `Offer` | The offer to calculate the deadline for. |

**Returns**

| Name     | Type     | Description                                      |
| -------- | -------- | ------------------------------------------------ |
| `<none>` | `uint64` | The deadline for the offer, as a UNIX timestamp. |

### lockDeadline

Calculates the lock deadline for the offer.

```solidity
function lockDeadline(Offer memory offer) internal pure returns (uint64);
```

**Parameters**

| Name    | Type    | Description                                   |
| ------- | ------- | --------------------------------------------- |
| `offer` | `Offer` | The offer to calculate the lock deadline for. |

**Returns**

| Name     | Type     | Description                                           |
| -------- | -------- | ----------------------------------------------------- |
| `<none>` | `uint64` | The lock deadline for the offer, as a UNIX timestamp. |

### eip712Digest

Computes the EIP-712 digest for the given offer.

```solidity
function eip712Digest(Offer memory offer) internal pure returns (bytes32);
```

**Parameters**

| Name    | Type    | Description                          |
| ------- | ------- | ------------------------------------ |
| `offer` | `Offer` | The offer to compute the digest for. |

**Returns**

| Name     | Type      | Description                      |
| -------- | --------- | -------------------------------- |
| `<none>` | `bytes32` | The EIP-712 digest of the offer. |
